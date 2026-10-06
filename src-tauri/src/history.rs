//! 最近完成的翻译，本地 DPAPI 加密保存，最多保留 50 条。

use std::{
    collections::{HashMap, HashSet},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::settings::dpapi;

pub const LIMIT: usize = 50;
const FILE_NAME: &str = "history.dpapi";
const FORMAT_VERSION: u32 = 1;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static STORES: OnceLock<Mutex<HashMap<PathBuf, HistoryState>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: String,
    pub created_at: u64,
    pub source: String,
    pub translation: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistorySnapshot {
    pub entries: Vec<HistoryEntry>,
    pub limit: usize,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredHistory {
    version: u32,
    entries: Vec<HistoryEntry>,
}

struct HistoryState {
    entries: Vec<HistoryEntry>,
    error: Option<String>,
    // 读取损坏文件时不覆盖它，防止一次新翻译毁掉仍可恢复的旧数据。
    read_error: Option<String>,
    dirty: bool,
}

impl HistoryState {
    fn snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            entries: self.entries.clone(),
            limit: LIMIT,
            error: self.error.clone(),
        }
    }
}

fn history_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(FILE_NAME))
        .map_err(|error| format!("获取历史记录目录失败：{error}"))
}

fn unavailable(error: String) -> HistorySnapshot {
    HistorySnapshot {
        entries: Vec::new(),
        limit: LIMIT,
        error: Some(error),
    }
}

pub fn snapshot(app: &AppHandle) -> HistorySnapshot {
    match history_path(app) {
        Ok(path) => snapshot_at(&path),
        Err(error) => unavailable(error),
    }
}

/// 只在流式翻译完整成功后调用；对齐请求失败不影响记录已有完整译文。
pub fn record(app: &AppHandle, source: &str, translation: &str) -> HistorySnapshot {
    match history_path(app) {
        Ok(path) => record_at(&path, source, translation),
        Err(error) => unavailable(error),
    }
}

pub fn find(app: &AppHandle, id: &str) -> Result<HistoryEntry, String> {
    let path = history_path(app)?;
    snapshot_at(&path)
        .entries
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| "这条历史记录已不存在，请刷新后重试".to_string())
}

fn with_state<T>(path: &Path, action: impl FnOnce(&mut HistoryState) -> T) -> T {
    // 同一进程内所有翻译任务共用一把锁：读、插入、截断、落盘是一次操作。
    let stores = STORES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut stores = stores.lock().unwrap_or_else(|error| error.into_inner());
    action(
        stores
            .entry(path.to_path_buf())
            .or_insert_with(|| load_state(path)),
    )
}

fn snapshot_at(path: &Path) -> HistorySnapshot {
    with_state(path, |state| {
        refresh_state(path, state);
        state.snapshot()
    })
}

/// UI 的刷新/重试是真正重新读取并重试保存，不让一次临时故障粘在缓存里。
fn refresh_state(path: &Path, state: &mut HistoryState) {
    match read_entries(path) {
        Ok(mut entries) => {
            if state.dirty {
                let mut seen = HashSet::new();
                entries = state
                    .entries
                    .iter()
                    .chain(entries.iter())
                    .filter(|entry| seen.insert(entry.id.clone()))
                    .cloned()
                    .collect();
                entries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
                entries.truncate(LIMIT);
            }
            state.entries = entries;
            state.read_error = None;
            state.error = None;
            if state.dirty {
                persist_state(path, state);
            }
        }
        Err(error) => {
            state.read_error = Some(error.clone());
            state.error = Some(if state.dirty {
                format!("{error}；新记录仅保存在内存中")
            } else {
                error
            });
        }
    }
}

fn persist_state(path: &Path, state: &mut HistoryState) {
    state.error = match &state.read_error {
        Some(error) => Some(format!("{error}；新记录仅保存在内存中")),
        None => match save_entries(path, &state.entries) {
            Ok(()) => {
                state.dirty = false;
                None
            }
            Err(error) => Some(format!("{error}；新记录仅保存在内存中")),
        },
    };
}

fn record_at(path: &Path, source: &str, translation: &str) -> HistorySnapshot {
    with_state(path, |state| {
        if source.trim().is_empty() || translation.trim().is_empty() {
            return state.snapshot();
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        state.entries.insert(
            0,
            HistoryEntry {
                id: format!("{}-{}-{sequence}", timestamp.as_nanos(), std::process::id()),
                created_at: timestamp.as_millis() as u64,
                source: source.to_string(),
                translation: translation.to_string(),
            },
        );
        state.entries.truncate(LIMIT);
        state.dirty = true;
        persist_state(path, state);
        state.snapshot()
    })
}

fn load_state(path: &Path) -> HistoryState {
    match read_entries(path) {
        Ok(entries) => HistoryState {
            entries,
            error: None,
            read_error: None,
            dirty: false,
        },
        Err(error) => HistoryState {
            entries: Vec::new(),
            error: Some(error.clone()),
            read_error: Some(error),
            dirty: false,
        },
    }
}

fn read_entries(path: &Path) -> Result<Vec<HistoryEntry>, String> {
    let encrypted = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取历史记录失败：{error}")),
    };
    let bytes = dpapi::unprotect(&encrypted)
        .ok_or_else(|| "历史记录无法解密，请确认正在使用原来的 Windows 用户".to_string())?;
    let mut stored: StoredHistory =
        serde_json::from_slice(&bytes).map_err(|error| format!("历史记录格式损坏：{error}"))?;
    if stored.version != FORMAT_VERSION {
        return Err("历史记录版本不受支持".to_string());
    }
    if stored.entries.iter().any(|entry| {
        entry.id.is_empty() || entry.source.trim().is_empty() || entry.translation.trim().is_empty()
    }) {
        return Err("历史记录包含不完整内容".to_string());
    }
    stored.entries.truncate(LIMIT);
    Ok(stored.entries)
}

struct PendingFile(PathBuf);

impl Drop for PendingFile {
    fn drop(&mut self) {
        // 仅清理本次以 create_new 创建的临时文件，不删除历史文件。
        let _ = std::fs::remove_file(&self.0);
    }
}

fn save_entries(path: &Path, entries: &[HistoryEntry]) -> Result<(), String> {
    let stored = StoredHistory {
        version: FORMAT_VERSION,
        entries: entries.to_vec(),
    };
    let bytes =
        serde_json::to_vec(&stored).map_err(|error| format!("序列化历史记录失败：{error}"))?;
    let encrypted = dpapi::protect(&bytes)?;
    let directory = path
        .parent()
        .ok_or_else(|| "历史记录路径无效".to_string())?;
    std::fs::create_dir_all(directory).map_err(|error| format!("创建历史记录目录失败：{error}"))?;

    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let temporary_path = directory.join(format!(
        ".{FILE_NAME}.{}.{sequence}.tmp",
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)
        .map_err(|error| format!("创建历史记录临时文件失败：{error}"))?;
    let temporary = PendingFile(temporary_path);
    file.write_all(&encrypted)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("写入历史记录失败：{error}"))?;
    drop(file);
    replace_file(&temporary.0, path).map_err(|error| format!("保存历史记录失败：{error}"))
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        },
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
        .map_err(std::io::Error::from)
    }
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "wordray-history-test-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&directory).unwrap();
            Self(directory)
        }

        fn path(&self) -> PathBuf {
            self.0.join(FILE_NAME)
        }

        fn forget_cache(&self) {
            if let Some(stores) = STORES.get() {
                stores.lock().unwrap().remove(&self.path());
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.forget_cache();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_history_starts_empty_and_incomplete_results_are_not_saved() {
        let fixture = Fixture::new();
        assert!(snapshot_at(&fixture.path()).entries.is_empty());
        record_at(&fixture.path(), "原文", "  ");
        record_at(&fixture.path(), "\n", "Translation");
        assert!(snapshot_at(&fixture.path()).entries.is_empty());
        assert!(!fixture.path().exists());
    }

    #[test]
    fn keeps_latest_fifty_complete_results_and_persists_order() {
        let fixture = Fixture::new();
        for index in 0..63 {
            assert!(record_at(
                &fixture.path(),
                &format!("原文 {index}"),
                &format!("Result {index}")
            )
            .error
            .is_none());
        }
        fixture.forget_cache();
        let snapshot = snapshot_at(&fixture.path());
        assert_eq!(snapshot.limit, 50);
        assert_eq!(snapshot.entries.len(), 50);
        assert_eq!(snapshot.entries.first().unwrap().source, "原文 62");
        assert_eq!(snapshot.entries.last().unwrap().translation, "Result 13");
        let ids: std::collections::HashSet<_> =
            snapshot.entries.iter().map(|entry| &entry.id).collect();
        assert_eq!(ids.len(), 50);
    }

    #[test]
    fn unicode_round_trips_and_disk_does_not_contain_plaintext() {
        let fixture = Fixture::new();
        let source = "  原文\n🦀 ‘Quoted’\\\"  ";
        let translation = "Translation\n🦀 with punctuation and spaces  ";
        let expected = record_at(&fixture.path(), source, translation);
        let encrypted = std::fs::read(fixture.path()).unwrap();
        assert!(!encrypted
            .windows(source.len())
            .any(|window| window == source.as_bytes()));
        assert!(!encrypted
            .windows(translation.len())
            .any(|window| window == translation.as_bytes()));
        fixture.forget_cache();
        let restored = snapshot_at(&fixture.path());
        assert!(restored.error.is_none());
        assert_eq!(restored.entries, expected.entries);
    }

    #[test]
    fn corrupt_file_is_preserved_and_new_complete_translation_remains_in_memory() {
        let fixture = Fixture::new();
        let damaged = b"damaged encrypted history";
        std::fs::write(fixture.path(), damaged).unwrap();
        assert!(snapshot_at(&fixture.path()).error.is_some());
        let snapshot = record_at(&fixture.path(), "新的原文", "New translation");
        assert_eq!(snapshot.entries.len(), 1);
        assert!(snapshot.error.unwrap().contains("仅保存在内存"));
        assert_eq!(std::fs::read(fixture.path()).unwrap(), damaged);
        assert_eq!(snapshot_at(&fixture.path()).entries.len(), 1);
    }

    #[test]
    fn failed_replacement_preserves_disk_and_next_save_recovers_pending_entries() {
        let fixture = Fixture::new();
        record_at(&fixture.path(), "已保存", "Saved");
        let old_bytes = std::fs::read(fixture.path()).unwrap();
        let mut permissions = std::fs::metadata(fixture.path()).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(fixture.path(), permissions).unwrap();
        let failed = record_at(&fixture.path(), "待保存", "Pending");
        assert_eq!(failed.entries.len(), 2);
        assert!(failed.error.is_some());
        assert_eq!(std::fs::read(fixture.path()).unwrap(), old_bytes);
        let mut permissions = std::fs::metadata(fixture.path()).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(fixture.path(), permissions).unwrap();
        assert!(record_at(&fixture.path(), "恢复保存", "Recovered")
            .error
            .is_none());
        fixture.forget_cache();
        assert_eq!(snapshot_at(&fixture.path()).entries.len(), 3);
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_completion_preserves_all_entries_without_duplicate_ids() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let tasks: Vec<_> = (0..32)
            .map(|index| {
                let path = path.clone();
                std::thread::spawn(move || {
                    record_at(&path, &format!("原文 {index}"), &format!("Result {index}"))
                })
            })
            .collect();
        for task in tasks {
            assert!(task.join().unwrap().error.is_none());
        }
        fixture.forget_cache();
        let snapshot = snapshot_at(&path);
        assert_eq!(snapshot.entries.len(), 32);
        let ids: std::collections::HashSet<_> =
            snapshot.entries.iter().map(|entry| &entry.id).collect();
        let sources: std::collections::HashSet<_> =
            snapshot.entries.iter().map(|entry| &entry.source).collect();
        assert_eq!(ids.len(), 32);
        assert_eq!(sources.len(), 32);
    }

    #[test]
    fn retry_after_repaired_read_error_merges_unsaved_entries() {
        let fixture = Fixture::new();
        let original = record_at(&fixture.path(), "旧原文", "Original");
        let old_bytes = std::fs::read(fixture.path()).unwrap();
        fixture.forget_cache();
        std::fs::write(fixture.path(), b"corrupt").unwrap();
        assert!(snapshot_at(&fixture.path()).error.is_some());
        assert!(record_at(&fixture.path(), "新原文", "Pending")
            .error
            .is_some());
        std::fs::write(fixture.path(), old_bytes).unwrap();

        let retried = snapshot_at(&fixture.path());
        assert!(retried.error.is_none());
        assert_eq!(retried.entries.len(), 2);
        assert!(retried
            .entries
            .iter()
            .any(|entry| entry.id == original.entries[0].id));
        assert!(retried
            .entries
            .iter()
            .any(|entry| entry.translation == "Pending"));
        fixture.forget_cache();
        assert_eq!(snapshot_at(&fixture.path()).entries, retried.entries);
    }

    #[test]
    fn retry_after_write_failure_saves_pending_without_a_new_translation() {
        let fixture = Fixture::new();
        record_at(&fixture.path(), "旧原文", "Original");
        let mut permissions = std::fs::metadata(fixture.path()).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(fixture.path(), permissions).unwrap();
        assert!(record_at(&fixture.path(), "新原文", "Pending")
            .error
            .is_some());
        let still_failed = snapshot_at(&fixture.path());
        assert!(still_failed.error.is_some());
        assert_eq!(still_failed.entries.len(), 2);

        let mut permissions = std::fs::metadata(fixture.path()).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(fixture.path(), permissions).unwrap();
        let retried = snapshot_at(&fixture.path());
        assert!(retried.error.is_none());
        assert_eq!(retried.entries.len(), 2);
        fixture.forget_cache();
        assert_eq!(snapshot_at(&fixture.path()).entries, retried.entries);
    }
}

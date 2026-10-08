//! 配置持久化：DeepSeek 的 base_url / model / API Key，以及用户选择的面板布局。
//!
//! **API Key 用 Windows DPAPI 加密后落盘**（`CryptProtectData`）：密文只有同一个
//! Windows 用户能解开，明文不写文件、不进日志、不进事件载荷。
//!
//! 取值优先级：**设置文件 > 环境变量 > 内置默认值**。
//! 设置是用户在界面上的明确意图，应当盖过环境变量；两者来源会在界面上标出来。

use std::{
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";
pub const DEFAULT_MODEL: &str = "deepseek-chat";

const CONFIG_FILE: &str = "config.json";
const LEGACY_IDENTIFIER: &str = "com.wuxian.selection-translator";
// 多个窗口可能同时读取设置，避免迁移写入期间读到不完整的文件。
static CONFIG_IO_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredSettings {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// DPAPI 加密后的密钥，hex 编码
    #[serde(default)]
    api_key_protected: Option<String>,
    /// 用户手动选择的布局；未选择时继续按原文长度自动决定。
    #[serde(default)]
    panel_wide: Option<bool>,
}

/// 密钥是从哪来的——界面上要如实显示，避免"我明明设了却不生效"的困惑
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Settings,
    Env,
    None,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub key_source: KeySource,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("拿不到配置目录：{e}"))?;
    Ok(dir.join(CONFIG_FILE))
}

fn read_stored(app: &AppHandle) -> StoredSettings {
    let Ok(path) = config_path(app) else {
        return StoredSettings::default();
    };
    read_stored_at_path(&path)
}

fn read_stored_at_path(path: &Path) -> StoredSettings {
    let _guard = CONFIG_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_stored_unlocked(path)
}

fn read_stored_unlocked(path: &Path) -> StoredSettings {
    match std::fs::read_to_string(path) {
        // 已有新配置（包括损坏的配置）都优先，不重新导入用户已清除的旧设置。
        Ok(text) => return serde_json::from_str(&text).unwrap_or_default(),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(_) => return StoredSettings::default(),
    }

    // Tauri 的 app_config_dir 默认是 config_dir / bundle_identifier。
    // WordRay 改名后仍可从同级旧标识目录导入设置，DPAPI 密文无需重新加密。
    let Some(legacy_path) = path
        .parent()
        .and_then(Path::parent)
        .map(|dir| dir.join(LEGACY_IDENTIFIER).join(CONFIG_FILE))
        .filter(|legacy_path| legacy_path != path)
    else {
        return StoredSettings::default();
    };
    let Ok(text) = std::fs::read_to_string(legacy_path) else {
        return StoredSettings::default();
    };
    let Ok(stored) = serde_json::from_str(&text) else {
        return StoredSettings::default();
    };

    match migrate_stored(path, &text) {
        // 另一个进程先建立了新配置，仍以新配置为准。
        Ok(false) => std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
        // 落盘失败时仍保留本次读取的旧设置，后续保存会写入新目录。
        Ok(true) | Err(_) => stored,
    }
}

/// 只创建缺失的新配置，不覆盖已有文件；保留旧 JSON 和密文字段原样。
fn migrate_stored(path: &Path, text: &str) -> std::io::Result<bool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(text.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(true)
}

/// 布局切换与设置窗口可能同时保存，整个读改写过程共用一把锁，防止相互覆盖。
fn update_stored_at_path(
    path: &Path,
    update: impl FnOnce(&mut StoredSettings) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = CONFIG_IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut stored = read_stored_unlocked(path);
    update(&mut stored)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(&stored).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("写入配置失败：{e}"))
}

pub fn load_panel_layout(app: &AppHandle) -> Option<bool> {
    read_stored(app).panel_wide
}

pub fn save_panel_layout(app: &AppHandle, wide: bool) -> Result<(), String> {
    update_stored_at_path(&config_path(app)?, |stored| {
        stored.panel_wide = Some(wide);
        Ok(())
    })
}

pub fn load(app: &AppHandle) -> Settings {
    let stored = read_stored(app);

    let from_settings = stored
        .api_key_protected
        .as_deref()
        .and_then(hex_decode)
        .and_then(|blob| dpapi::unprotect(&blob))
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|key| !key.trim().is_empty());

    let (api_key, key_source) = match from_settings {
        Some(key) => (Some(key), KeySource::Settings),
        None => match std::env::var("DEEPSEEK_API_KEY") {
            Ok(key) if !key.trim().is_empty() => (Some(key), KeySource::Env),
            _ => (None, KeySource::None),
        },
    };

    let pick = |stored_value: Option<String>, env_name: &str, fallback: &str| -> String {
        stored_value
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var(env_name)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
            })
            .unwrap_or_else(|| fallback.to_string())
    };

    Settings {
        base_url: pick(stored.base_url, "DEEPSEEK_BASE_URL", DEFAULT_BASE_URL),
        model: pick(stored.model, "DEEPSEEK_MODEL", DEFAULT_MODEL),
        api_key,
        key_source,
    }
}

/// 保存设置。
///
/// `api_key` 语义：
/// - `None`  → 不改动已保存的密钥
/// - `Some("")` → 清除密钥
/// - `Some(key)` → 加密后保存
pub fn save(
    app: &AppHandle,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(), String> {
    update_stored_at_path(&config_path(app)?, |stored| {
        stored.base_url = Some(base_url.trim().to_string());
        stored.model = Some(model.trim().to_string());

        if let Some(key) = api_key {
            let key = key.trim();
            if key.is_empty() {
                stored.api_key_protected = None;
            } else {
                let blob = dpapi::protect(key.as_bytes())?;
                stored.api_key_protected = Some(hex_encode(&blob));
            }
        }
        Ok(())
    })
}

/// 配置文件路径，仅供界面显示（方便用户知道东西存在哪）
pub fn describe_path(app: &AppHandle) -> String {
    config_path(app)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|err| err)
}

// ---------------------------------------------------------------- hex

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

// ---------------------------------------------------------------- DPAPI

pub(crate) mod dpapi {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn free_blob(blob: &CRYPT_INTEGER_BLOB) {
        if !blob.pbData.is_null() {
            unsafe {
                // LocalFree 在 windows-rs 里的签名是 `Param<HLOCAL>`，
                // 指针类句柄要**直接传** HLOCAL，包成 Some(..) 反而不满足 trait
                let _ = LocalFree(HLOCAL(blob.pbData as *mut core::ffi::c_void));
            }
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        unsafe {
            CryptProtectData(&mut input, None, None, None, None, 0, &mut output)
                .map_err(|e| format!("DPAPI 加密失败：{e}"))?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            free_blob(&output);
            Ok(bytes)
        }
    }

    pub fn unprotect(blob: &[u8]) -> Option<Vec<u8>> {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        unsafe {
            CryptUnprotectData(&mut input, None, None, None, None, 0, &mut output).ok()?;
            let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            free_blob(&output);
            Some(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SettingsFixture(PathBuf);

    impl SettingsFixture {
        fn new() -> Self {
            static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "wordray-settings-test-{}-{timestamp}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }

        fn current_path(&self) -> PathBuf {
            self.0.join("com.wuxian.wordray").join(CONFIG_FILE)
        }

        fn legacy_path(&self) -> PathBuf {
            self.0.join(LEGACY_IDENTIFIER).join(CONFIG_FILE)
        }

        fn write(&self, path: &Path, text: &str) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for SettingsFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const LEGACY_SETTINGS: &str = r#"{
        "base_url": "https://example.test/v1",
        "model": "legacy-model",
        "api_key_protected": "00112233aabbccdd",
        "future_preference": true
    }"#;

    #[test]
    fn renamed_app_imports_legacy_settings_and_preserves_ciphertext() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&fixture.legacy_path(), LEGACY_SETTINGS);

        let stored = read_stored_at_path(&current);

        assert_eq!(stored.base_url.as_deref(), Some("https://example.test/v1"));
        assert_eq!(stored.model.as_deref(), Some("legacy-model"));
        assert_eq!(
            stored.api_key_protected.as_deref(),
            Some("00112233aabbccdd")
        );
        assert_eq!(std::fs::read_to_string(&current).unwrap(), LEGACY_SETTINGS);
        assert_eq!(
            std::fs::read_to_string(fixture.legacy_path()).unwrap(),
            LEGACY_SETTINGS
        );
    }

    #[test]
    fn existing_new_settings_keep_cleared_key_and_preferences() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        let current_text = r#"{"model":"new-model","api_key_protected":null}"#;
        fixture.write(&fixture.legacy_path(), LEGACY_SETTINGS);
        fixture.write(&current, current_text);

        let stored = read_stored_at_path(&current);

        assert_eq!(stored.model.as_deref(), Some("new-model"));
        assert!(stored.api_key_protected.is_none());
        assert!(stored.base_url.is_none());
        assert!(stored.panel_wide.is_none());
        assert_eq!(std::fs::read_to_string(current).unwrap(), current_text);
    }

    #[test]
    fn panel_layout_survives_reload_and_other_settings_changes() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&current, LEGACY_SETTINGS);

        for wide in [true, false] {
            update_stored_at_path(&current, |stored| {
                stored.panel_wide = Some(wide);
                Ok(())
            })
            .unwrap();
            // 模拟设置窗口之后保存模型，不能覆盖已选择的布局或已加密密钥。
            update_stored_at_path(&current, |stored| {
                stored.model = Some("updated-model".to_string());
                Ok(())
            })
            .unwrap();

            let reloaded = read_stored_at_path(&current);
            assert_eq!(reloaded.panel_wide, Some(wide));
            assert_eq!(reloaded.model.as_deref(), Some("updated-model"));
            assert_eq!(
                reloaded.api_key_protected.as_deref(),
                Some("00112233aabbccdd")
            );
        }
    }

    #[test]
    fn corrupt_new_settings_do_not_restore_legacy_preferences() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&fixture.legacy_path(), LEGACY_SETTINGS);
        fixture.write(&current, "invalid JSON");

        let stored = read_stored_at_path(&current);

        assert!(stored.model.is_none());
        assert!(stored.api_key_protected.is_none());
        assert_eq!(std::fs::read_to_string(current).unwrap(), "invalid JSON");
    }

    #[test]
    fn corrupt_legacy_settings_are_not_migrated() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&fixture.legacy_path(), "invalid JSON");

        let stored = read_stored_at_path(&current);

        assert!(stored.model.is_none());
        assert!(!current.exists());
    }

    #[test]
    fn missing_legacy_settings_keep_default_and_do_not_create_config() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();

        let stored = read_stored_at_path(&current);

        assert!(stored.model.is_none());
        assert!(!current.exists());
    }

    #[test]
    fn migration_never_overwrites_a_newly_created_config() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&current, "current contents");

        assert!(!migrate_stored(&current, LEGACY_SETTINGS).unwrap());
        assert_eq!(
            std::fs::read_to_string(current).unwrap(),
            "current contents"
        );
    }

    #[test]
    fn migration_is_not_repeated_after_legacy_changes() {
        let fixture = SettingsFixture::new();
        let current = fixture.current_path();
        fixture.write(&fixture.legacy_path(), LEGACY_SETTINGS);
        read_stored_at_path(&current);
        fixture.write(&fixture.legacy_path(), r#"{"model":"changed-old-model"}"#);

        let stored = read_stored_at_path(&current);

        assert_eq!(stored.model.as_deref(), Some("legacy-model"));
        assert_eq!(std::fs::read_to_string(current).unwrap(), LEGACY_SETTINGS);
    }

    #[test]
    fn hex_roundtrip() {
        let data: Vec<u8> = (0u8..=255).collect();
        let encoded = hex_encode(&data);
        assert_eq!(encoded.len(), data.len() * 2);
        assert_eq!(hex_decode(&encoded).unwrap(), data);
    }

    #[test]
    fn hex_decode_rejects_bad_input() {
        assert!(hex_decode("abc").is_none()); // 奇数长度
        assert!(hex_decode("zz").is_none()); // 非十六进制
    }

    #[test]
    fn dpapi_roundtrip() {
        let plain = b"sk-test-not-a-real-key";
        let protected = dpapi::protect(plain).expect("加密应当成功");
        assert_ne!(&protected[..], &plain[..], "密文不应等于明文");
        let restored = dpapi::unprotect(&protected).expect("解密应当成功");
        assert_eq!(restored, plain);
    }
}

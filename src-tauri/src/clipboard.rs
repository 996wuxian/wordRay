//! 取词：向当前应用注入 Ctrl+C，再把剪贴板内容读出来，最后尽量还原。
//!
//! 这条路径现在是**备选**：主路径走 UI Automation（见 `uia.rs`，完全不碰剪贴板）。
//! 只有当 UIA 读不到选区、且该应用从未证明过 UIA 可用时，才退到这里。
//! 微信这类不暴露 UIA 文本的应用只能靠它。
//!
//! 已知限制（阶段 3 处理）：
//! - 只能保存/还原**文本**格式；剪贴板里原本是图片等非文本内容时会被这次取词覆盖。
//! - 尚未实现"按下快照"来区分"拖动一个已选中的元素"与真划词（需要额外判定）。

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::POINT;
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_C, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetCursorPos, GetForegroundWindow};

/// 这些前台窗口类里绝不注入 Ctrl+C。
///
/// 前四条是终端：那里 Ctrl+C 是中断信号。
/// 后几条是**"Ctrl+C 复制出来的不是文字、却会在剪贴板里留下文本"**的重灾区：
/// 资源管理器里 Ctrl+C 复制的是文件，剪贴板会挂一条路径文本——
/// 不挡掉的话，拖一个文件就会冒出一个翻译图标。
const NO_INJECTION_CLASSES: &[&str] = &[
    // 终端
    "ConsoleWindowClass",            // conhost：cmd / PowerShell
    "CASCADIA_HOSTING_WINDOW_CLASS", // Windows Terminal
    "mintty",                        // Git Bash
    "PuTTY",
    // 文件管理器与桌面
    "CabinetWClass",
    "ExploreWClass",
    "Progman",
    "WorkerW",
];

/// 等修饰键松开的上限。热键按下的那一刻修饰键必然还按着，必须等。
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_millis(900);

/// 注入 Ctrl+C 之后等剪贴板被写入的上限
const CLIPBOARD_WAIT: Duration = Duration::from_millis(600);

#[derive(Debug)]
pub enum CaptureError {
    ModifiersStillHeld,
    UserCopying,
    BlockedClass(String),
    NoNewText,
    Clipboard(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModifiersStillHeld => write!(
                f,
                "热键的 Ctrl / Alt 还没松开，本次跳过以免打断你的组合键。松开后重按一次即可。"
            ),
            Self::UserCopying => {
                write!(f, "检测到你正按着 C，本次跳过，避免和你自己的复制抢剪贴板")
            }
            Self::BlockedClass(class) => {
                write!(f, "当前应用（窗口类 {class}）在禁止注入名单里，已跳过取词")
            }
            Self::NoNewText => write!(f, "没有读到选中的文字：请先选中文本，再按热键"),
            Self::Clipboard(message) => write!(f, "剪贴板访问失败：{message}"),
        }
    }
}

fn open_clipboard() -> Result<arboard::Clipboard, CaptureError> {
    arboard::Clipboard::new().map_err(|e| CaptureError::Clipboard(e.to_string()))
}

pub fn read_text() -> Option<String> {
    open_clipboard().ok().and_then(|mut c| c.get_text().ok())
}

pub fn write_text(text: &str) -> Result<(), String> {
    let mut clipboard = open_clipboard().map_err(|e| e.to_string())?;
    clipboard
        .set_text(text.to_string())
        .map_err(|e| e.to_string())
}

/// 剪贴板的写入序号：系统每写一次剪贴板就 +1。
/// 这是判断"这次注入的 Ctrl+C 有没有真的发生复制"的唯一可靠依据。
fn clipboard_sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

fn is_key_down(virtual_key: VIRTUAL_KEY) -> bool {
    // GetAsyncKeyState 的最高位为 1 表示当前按下
    unsafe { GetAsyncKeyState(virtual_key.0 as i32) < 0 }
}

/// 等用户松开热键的修饰键。
///
/// 为什么必须等：热键是 Ctrl+Alt+T，按下的瞬间 Ctrl 与 Alt 一定还按着。
/// 此时若注入 Ctrl+C，我们自己发的 Ctrl 抬起事件会把用户**尚未松开**的组合键拆散
/// （应用看到的是"一个没有 Ctrl 的 c"）。
fn wait_for_modifiers_release(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if !is_key_down(VK_CONTROL) && !is_key_down(VK_MENU) && !is_key_down(VK_SHIFT) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(15));
    }
}

pub fn foreground_class_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut buffer = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buffer);
        if len <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

pub fn cursor_position() -> Option<(i32, i32)> {
    unsafe {
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            Some((point.x, point.y))
        } else {
            None
        }
    }
}

fn key_input(virtual_key: VIRTUAL_KEY, key_up: bool) -> INPUT {
    // 必须带扫描码：Chromium / WebView2 一类窗口不认 wScan = 0 的合成按键
    let scan = unsafe { MapVirtualKeyW(virtual_key.0 as u32, MAPVK_VK_TO_VSC) as u16 };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_ctrl_c() {
    let inputs = [
        key_input(VK_CONTROL, false),
        key_input(VK_C, false),
        key_input(VK_C, true),
        key_input(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 取词主流程：等修饰键松开 → 存原文 → 注入 Ctrl+C → 等剪贴板序号变化 → 条件还原
pub fn capture_selection() -> Result<String, CaptureError> {
    if !wait_for_modifiers_release(MODIFIER_RELEASE_TIMEOUT) {
        return Err(CaptureError::ModifiersStillHeld);
    }

    // 用户自己正按着 C（例如刚按了 Ctrl+C）：绝不注入，否则会抢他的剪贴板
    if is_key_down(VK_C) {
        return Err(CaptureError::UserCopying);
    }

    if let Some(class) = foreground_class_name() {
        if NO_INJECTION_CLASSES
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(&class))
        {
            return Err(CaptureError::BlockedClass(class));
        }
    }

    let before_text = read_text();
    let before_sequence = clipboard_sequence();

    send_ctrl_c();

    // 用**写入序号**判断这次复制有没有真的发生，而不是比较内容。
    // 比较内容是错的：如果应用把同一段文字再复制一次，内容完全没变，
    // 会被误判成"没复制"，于是漏掉一次正常划词。
    let deadline = Instant::now() + CLIPBOARD_WAIT;
    let mut captured: Option<(String, u32)> = None;
    while Instant::now() < deadline {
        sleep(Duration::from_millis(20));

        if clipboard_sequence() != before_sequence {
            // 序号变了，稍等一下让内容落定
            sleep(Duration::from_millis(25));
            if let Some(text) = read_text() {
                if !text.trim().is_empty() {
                    captured = Some((text, clipboard_sequence()));
                    break;
                }
            }
        }
    }

    let (text, sequence_at_capture) = captured.ok_or(CaptureError::NoNewText)?;

    // 事务化还原：只有在"剪贴板仍是这次刚读到的那一份、且没人再写过"时才写回，
    // 否则可能踩掉用户在此期间自己复制的东西。
    if let Some(previous) = before_text {
        if clipboard_sequence() == sequence_at_capture
            && read_text().as_deref() == Some(text.as_str())
        {
            let _ = write_text(&previous);
        }
    }

    Ok(text)
}

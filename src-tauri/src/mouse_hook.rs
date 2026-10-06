//! 全局低级鼠标钩子（`WH_MOUSE_LL`）。
//!
//! 钩子回调运行在安装它的线程上，且**必须立刻返回**——系统对低级钩子有超时，
//! 回调里拖泥带水会被静默摘掉钩子。所以这里只做一件事：把原始事件丢进通道。
//! 任何 UIA 调用都必须留在别的线程上做。

use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, HC_ACTION, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
};

use crate::gesture::{RawEvent, RawKind};

static SENDER: OnceLock<Sender<RawEvent>> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

fn elapsed_ms() -> u64 {
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_millis() as u64
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kind = match wparam.0 as u32 {
            WM_LBUTTONDOWN => Some(RawKind::Down),
            WM_LBUTTONUP => Some(RawKind::Up),
            _ => None,
        };

        if let Some(kind) = kind {
            // SAFETY: 对 WM_LBUTTON* 而言 lparam 一定是 MSLLHOOKSTRUCT
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            if let Some(sender) = SENDER.get() {
                let _ = sender.send(RawEvent {
                    kind,
                    x: info.pt.x,
                    y: info.pt.y,
                    t_ms: elapsed_ms(),
                });
            }
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 安装钩子并进入消息循环。**必须在它自己的线程上调用**，正常情况下不会返回。
pub fn run(sender: Sender<RawEvent>) -> Result<(), String> {
    SENDER
        .set(sender)
        .map_err(|_| "鼠标钩子已经安装过了".to_string())?;

    unsafe {
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), None, 0)
            .map_err(|e| format!("安装全局鼠标钩子失败：{e}"))?;

        println!("[WordRay] 全局鼠标钩子已安装");

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = UnhookWindowsHookEx(hook);
    }

    Ok(())
}

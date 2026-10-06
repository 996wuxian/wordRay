#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod deepseek;
mod gesture;
mod history;
mod mouse_hook;
mod selection_alignment;
mod settings;
mod uia;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::thread;

use gesture::{Gesture, GestureDetector, RawEvent};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// 每次翻译用自增 session id 标识，前端据此丢弃迟到的旧结果
static SESSION: AtomicU64 = AtomicU64::new(0);

/// 启动时实际注册成功的热键标签，供前端显示；一个都没成功时为 None
static ACTIVE_HOTKEY: Mutex<Option<String>> = Mutex::new(None);

/// 划词识别到的选区，等用户点图标时使用
static PENDING: Mutex<Option<Selection>> = Mutex::new(None);

/// 用户是否已经把面板拖到了自定义位置。
///
/// 一旦为真，`show_panel` 就不再自动挪动它。
/// 不加这个的话，拖完下一次翻译面板就被挪回选区旁边，拖动等于白做。
static PANEL_PINNED: Mutex<bool> = Mutex::new(false);

/// 面板当前是不是宽版（原文左 / 译文右）。
///
/// 由 Rust 侧持有：窗口尺寸是 Rust 在改，前端只负责按这个标志渲染。
static PANEL_WIDE: Mutex<bool> = Mutex::new(false);

const ICON_SIZE: i32 = 40;

/// 面板窄版：竖排（原文在上、译文在下）
const PANEL_NARROW_W: i32 = 440;
const PANEL_NARROW_H: i32 = 280;

/// 面板宽版：左右分栏（原文在左、译文在右）
const PANEL_WIDE_W: i32 = 860;
const PANEL_WIDE_H: i32 = 300;

/// 原文超过这个字数（或含换行）就自动用宽版。
/// 窄版一屏大约放得下 100 来字，再多就要滚动，所以分界线放在这里。
const WIDE_LAYOUT_THRESHOLD: usize = 80;

/// 宽版最多占显示器的这个比例宽度，避免在小屏上顶到屏幕外
const WIDE_MAX_SCREEN_RATIO: f64 = 0.72;

fn wants_wide_layout(text: &str) -> bool {
    text.chars().count() > WIDE_LAYOUT_THRESHOLD || text.contains('\n')
}

fn panel_size(wide: bool) -> (i32, i32) {
    if wide {
        (PANEL_WIDE_W, PANEL_WIDE_H)
    } else {
        (PANEL_NARROW_W, PANEL_NARROW_H)
    }
}

/// 对齐表的分隔符。模型在译文之后输出它，之后才是对齐 JSON。
/// 界面只显示分隔符之前的内容（见 `deepseek::stream_chat_split`）。
const ALIGN_MARKER: &str = "#ALIGN#";

const SYSTEM_PROMPT: &str = concat!(
    "你是翻译引擎。把用户输入的中文翻译成自然、地道的英文。保留专有名词、代码与术语不译。\n",
    "\n",
    "输出格式（严格遵守）：\n",
    "先只输出译文，不要解释、不要加引号。\n",
    "然后单独一行输出：#ALIGN#\n",
    "然后输出一个 JSON 数组，把原文切成**细粒度**片段，并给出每段在译文中对应的片段：\n",
    "[{\"src\":\"原文片段\",\"dst\":\"译文片段\"}]\n",
    "约束：\n",
    "- src 必须是原文的连续子串；按顺序拼接后要等于原文，不得漏字或改写\n",
    "- dst 必须是译文的连续子串。**英文语序与中文不同，所以 dst 不要求和 src 同序**\n",
    "- 切分必须按最小语义词：例如「上词对齐模型」拆成「上」「词对齐」「模型」，「模型」只对应 model；\n",
    "  两三字的词也要单独成段，不要把多个独立词合并成一个词组；\n",
    "  绝不能整句一整段地给，否则用户选中一个词时定位不到对应的英文\n",
    "- 片段数量与原文词数成正比，不要为了减少段数而合并独立词\n",
    "- #ALIGN# 之后除了那个 JSON 数组，不要再输出任何内容\n",
    "\n",
    "粒度示例（只看切分粗细，注意 dst 与 src 语序不同）：\n",
    "原文：这个功能以后会加上。\n",
    "译文：This feature will be added later.\n",
    "[{\"src\":\"这个\",\"dst\":\"This\"},{\"src\":\"功能\",\"dst\":\"feature\"},{\"src\":\"以后\",\"dst\":\"later\"},",
    "{\"src\":\"会加上\",\"dst\":\"will be added\"}]"
);

#[derive(Clone)]
struct Selection {
    text: String,
    rect: Option<uia::ScreenRect>,
}

#[derive(Clone, Serialize)]
struct StatePayload {
    hotkey: Option<String>,
    panel_pinned: bool,
    panel_wide: bool,
}

/// 面板形态变化时推给前端。窗口尺寸由 Rust 改，前端只需要跟着换布局。
#[derive(Clone, Serialize)]
struct LayoutPayload {
    wide: bool,
}

#[derive(Clone, Serialize)]
struct StartPayload {
    session_id: u64,
    source: String,
}

#[derive(Clone, Serialize)]
struct DeltaPayload {
    session_id: u64,
    text: String,
}

#[derive(Clone, Serialize)]
struct DonePayload {
    session_id: u64,
    full_text: String,
}

#[derive(Clone, Serialize)]
struct ErrorPayload {
    session_id: u64,
    message: String,
}

/// 历史窗口按通知重新读取，避免向每个窗口广播完整的 50 条正文。
#[derive(Clone, Serialize)]
struct HistoryChangedPayload {
    error: Option<String>,
}

/// 对齐表的一项：原文片段 ↔ 译文片段（都是字面子串）
#[derive(Clone, Serialize, serde::Deserialize)]
struct AlignPair {
    src: String,
    dst: String,
}

#[derive(Clone, Serialize)]
struct AlignPayload {
    session_id: u64,
    pairs: Vec<AlignPair>,
}

/// 从模型给的尾段里抠出对齐表。
///
/// 不假设它只输出 JSON：模型常会包一层 ```json 代码块，或者前后带客套话。
/// 所以从第一个 `[` 截到最后一个 `]`，其余一律忽略；解析失败返回 None。
fn parse_alignment(text: &str) -> Option<Vec<AlignPair>> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end <= start {
        return None;
    }
    let pairs: Vec<AlignPair> = serde_json::from_str(&text[start..=end]).ok()?;
    if pairs.is_empty() {
        return None;
    }
    Some(pairs)
}

/// src 片段超过这么多字，就认为"切得太粗"——用户选中一个词时会对不上。
const ALIGN_MAX_SRC_CHARS: usize = 10;

/// 校验对齐表：算出**能定位到**的对数，以及最长的 src 片段字数。
///
/// 定位规则必须与前端 `selectionAlign.ts` 保持一致：
/// src 单调前进；dst 先按当前位置找、找不到再全局找（英文语序与中文不同）。
/// 否则后端认为"可用"、前端却定位不到，两边判断打架。
fn assess_alignment(source: &str, translation: &str, pairs: &[AlignPair]) -> (usize, usize) {
    let mut src_cursor = 0usize;
    let mut dst_cursor = 0usize;
    let mut usable = 0usize;
    let mut longest_src = 0usize;

    for pair in pairs {
        if pair.src.is_empty() || pair.dst.is_empty() {
            continue;
        }

        let Some(offset) = source[src_cursor..].find(&pair.src) else {
            continue;
        };
        let src_at = src_cursor + offset;

        let dst_at = translation[dst_cursor..]
            .find(&pair.dst)
            .map(|i| dst_cursor + i)
            .or_else(|| translation.find(&pair.dst));
        let Some(dst_at) = dst_at else {
            continue;
        };

        usable += 1;
        longest_src = longest_src.max(pair.src.chars().count());
        src_cursor = src_at + pair.src.len();
        if dst_at >= dst_cursor {
            dst_cursor = dst_at + pair.dst.len();
        }
    }

    (usable, longest_src)
}

/// 对齐表够不够用：至少有一对能定位、粒度够细、且大部分对能定位。
fn alignment_is_good(usable: usize, longest_src: usize, total: usize) -> bool {
    usable > 0 && longest_src <= ALIGN_MAX_SRC_CHARS && usable * 10 >= total * 6
}

/// 专门要一次对齐表。
///
/// 为什么要有这条路：把对齐表塞进翻译输出里是"零额外请求"，但模型经常不守格式
/// 或者切得很粗。校验不过就退到这次**只做对齐**的请求——任务单一，遵守率明显更高。
const ALIGN_SYSTEM_PROMPT: &str = concat!(
    "你是翻译对齐标注器。用户会给你一段中文原文和它的英文译文。\n",
    "把原文切成最小语义词（如「词对齐模型」拆成「词对齐」「模型」，「模型」只对应 model），\n",
    "再给出每个片段在译文中对应的片段，输出一个 JSON 数组：\n",
    "[{\"src\":\"原文片段\",\"dst\":\"译文片段\"}]\n",
    "约束：\n",
    "- src 必须是原文的连续子串；按顺序拼接后等于原文，不得漏字或改写\n",
    "- dst 必须是译文的连续子串；英文语序与中文不同，所以 dst 不要求和 src 同序\n",
    "- 每个独立词单独成段，不要为了减少段数而合并词组\n",
    "- 只输出那个 JSON 数组，不要解释，不要用代码块包裹"
);

async fn request_alignment(
    config: &deepseek::Config,
    source: &str,
    translation: &str,
) -> Result<Vec<AlignPair>, String> {
    let user = format!("原文：\n{source}\n\n译文：\n{translation}");
    let reply = deepseek::stream_chat(config, Some(ALIGN_SYSTEM_PROMPT), &user, |_| {}).await?;
    parse_alignment(&reply).ok_or_else(|| "对齐 JSON 解析失败".to_string())
}

fn next_session() -> u64 {
    SESSION.fetch_add(1, Ordering::SeqCst) + 1
}

// ---------------------------------------------------------------- 窗口

fn window_contains(win: &WebviewWindow, x: i32, y: i32) -> bool {
    if !win.is_visible().unwrap_or(false) {
        return false;
    }
    let (Ok(position), Ok(size)) = (win.outer_position(), win.outer_size()) else {
        return false;
    };
    x >= position.x
        && x < position.x + size.width as i32
        && y >= position.y
        && y < position.y + size.height as i32
}

fn point_in_window(app: &AppHandle, label: &str, point: (i32, i32)) -> bool {
    app.get_webview_window(label)
        .map(|win| window_contains(&win, point.0, point.1))
        .unwrap_or(false)
}

/// 把目标位置夹到它所在显示器的可视范围内，避免弹到屏幕外面去
fn clamp_to_monitor(app: &AppHandle, x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let Ok(Some(monitor)) = app.monitor_from_point(x as f64, y as f64) else {
        return (x, y);
    };
    let origin = monitor.position();
    let size = monitor.size();
    let min_x = origin.x + 8;
    let min_y = origin.y + 8;
    let max_x = (origin.x + size.width as i32 - w - 8).max(min_x);
    let max_y = (origin.y + size.height as i32 - h - 8).max(min_y);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

/// 让图标窗真的能小到 40×40。
///
/// **关键坑（实测）**：即便配了 `decorations: false`，tao/Tauri 仍然保留 `WS_CAPTION`
/// 与 `WS_SYSMENU`，于是 Windows 会把窗口夹到系统最小尺寸
/// `SM_CXMINTRACK × SM_CYMINTRACK`（本机实测 136×39）。
/// 表现就是"配置写的 40×40，实际量出来 135×40"——多出来的宽度是系统的下限，不是布局问题。
///
/// 去掉这些样式位之后窗口才能真正变小；`SWP_FRAMECHANGED` 让改动立刻生效。
#[cfg(windows)]
fn make_icon_window_small(win: &WebviewWindow) {
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
        SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };

    let Ok(handle) = win.hwnd() else {
        return;
    };
    // Tauri 用的 windows crate 版本与本项目不同，跨版本只能经由裸指针转一次
    let hwnd = HWND(handle.0 as isize as *mut core::ffi::c_void);

    // 顺序很重要：先让 Tauri 把该设的都设完。
    // 这些调用内部会按 tao 记录的窗口标志**重建整套窗口样式**，
    // 如果把 Win32 样式手术放在它们前面，刚剥掉的 WS_CAPTION 会被加回来（实测踩过）。
    let _ = win.set_shadow(false);
    let _ = win.set_resizable(false);
    let _ = win.set_size(tauri::LogicalSize::new(ICON_SIZE as f64, ICON_SIZE as f64));

    // 最后才动样式位
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let strip = (WS_CAPTION.0 as isize)
            | (WS_SYSMENU.0 as isize)
            | (WS_MINIMIZEBOX.0 as isize)
            | (WS_MAXIMIZEBOX.0 as isize)
            | (WS_THICKFRAME.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style & !strip);

        // 不抢焦点 + 不进 Alt+Tab。
        // 点图标不该把原应用的焦点夺走——选中文字在划词那一刻就已经读好了，不需要焦点。
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let add = (WS_EX_NOACTIVATE.0 as isize) | (WS_EX_TOOLWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | add);

        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
        );

        // 真正的圆角：用窗口区域把窗口裁成圆角矩形。
        //
        // 为什么不能只靠 CSS 的 border-radius：实测本机透明没有生效
        // （可见状态下 ex-style 里既无 WS_EX_LAYERED 也无 WS_EX_NOREDIRECTIONBITMAP），
        // 于是圆角外面那部分露的是窗口自己的白底，看起来就是
        // "白方块里画了个圆角边框"的假圆角。区域裁剪是真的把角切掉。
        //
        // 代价：区域边缘没有抗锯齿，圆角处会略有锯齿。圆角半径取宽度的 1/4，
        // 与 icon.css 里的 border-radius: 10px / 40px 对齐。
        let size = win
            .outer_size()
            .unwrap_or(tauri::PhysicalSize::new(ICON_SIZE as u32, ICON_SIZE as u32));
        let radius = ((size.width as f32 * 0.25) as i32).max(4);
        let region = CreateRoundRectRgn(
            0,
            0,
            size.width as i32 + 1,
            size.height as i32 + 1,
            radius * 2,
            radius * 2,
        );
        if !region.is_invalid() {
            // 成功后区域归系统所有，不能再 DeleteObject；失败才需要自己释放
            if SetWindowRgn(hwnd, region, BOOL(1)) == 0 {
                let _ = DeleteObject(HGDIOBJ(region.0));
            }
        }

        let final_style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let final_ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        println!(
            "[WordRay] 图标窗 style=0x{final_style:X} ex=0x{final_ex:X} caption_removed={} noactivate={}",
            (final_style & (WS_CAPTION.0 as isize)) == 0,
            (final_ex & (WS_EX_NOACTIVATE.0 as isize)) != 0
        );
    }
}

/// 让图标窗真的能小到 40×40。非 Windows 平台不做任何事。
#[cfg(not(windows))]
fn make_icon_window_small(_win: &WebviewWindow) {}

fn show_icon(app: &AppHandle, anchor_x: i32, anchor_y: i32) {
    let Some(win) = app.get_webview_window("icon") else {
        return;
    };

    // anchor 是选区右边缘与垂直中线，图标挂在它右侧
    let (x, y) = clamp_to_monitor(
        app,
        anchor_x + 8,
        anchor_y - ICON_SIZE / 2,
        ICON_SIZE,
        ICON_SIZE,
    );

    // 先摆好位置、再显示
    let _ = win.set_position(PhysicalPosition::new(x, y));
    let _ = win.show();

    // 样式手术必须放在 show() **之后**。
    // tao 在 show() 时会按它内部记录的窗口标志重建整套 style / ex-style，
    // 放在前面会被整套覆盖回去。实测表现：隐藏时样式是对的，一显示
    // WS_CAPTION 就回来了（于是 DWM 又给它画投影），WS_EX_NOACTIVATE 也没了。
    make_icon_window_small(&win);

    println!(
        "[WordRay] 图标窗 outer={:?} inner={:?} scale={:?}",
        win.outer_size(),
        win.inner_size(),
        win.scale_factor()
    );
}

fn hide_icon(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("icon") {
        let _ = win.hide();
    }
}

/// 应用面板形态：改窗口尺寸、记住状态、通知前端。返回实际使用的尺寸
/// （宽版可能被显示器宽度限制得更窄）。
fn apply_panel_size(app: &AppHandle, wide: bool, probe: (i32, i32)) -> (i32, i32) {
    let (mut width, height) = panel_size(wide);

    if wide {
        if let Ok(Some(monitor)) = app.monitor_from_point(probe.0 as f64, probe.1 as f64) {
            let cap = (monitor.size().width as f64 * WIDE_MAX_SCREEN_RATIO) as i32;
            width = width.min(cap.max(PANEL_NARROW_W));
        }
    }

    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.set_size(tauri::LogicalSize::new(width as f64, height as f64));
    }

    if let Ok(mut guard) = PANEL_WIDE.lock() {
        *guard = wide;
    }

    // 打出决策结果：「为什么这次没变宽」是最容易被问的问题，日志里要能直接看到
    println!(
        "[WordRay] 面板形态：{}（{}x{}）",
        if wide { "左右分栏" } else { "竖排" },
        width,
        height
    );

    let _ = app.emit("panel://layout", LayoutPayload { wide });

    (width, height)
}

fn show_panel(app: &AppHandle, selection_rect: Option<uia::ScreenRect>, wide: bool) {
    let Some(win) = app.get_webview_window("panel") else {
        return;
    };

    let probe = match selection_rect {
        Some((left, top, width, height)) => ((left + width) as i32 + 12, (top + height) as i32 + 6),
        None => clipboard::cursor_position()
            .map(|(x, y)| (x + 18, y + 18))
            .unwrap_or((240, 240)),
    };

    let (width, height) = apply_panel_size(app, wide, probe);

    // 用户拖过之后就不再自动挪动它，否则"拖动"这个功能等于没用
    let pinned = PANEL_PINNED.lock().map(|value| *value).unwrap_or(false);
    if !pinned {
        let (x, y) = clamp_to_monitor(app, probe.0, probe.1, width, height);
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }

    let _ = win.show();
}

/// 托盘恢复只显示已有面板，保留当前译文、尺寸和位置。
/// `open_panel` 专用于划词浮标，会消费选区并发起新翻译，不能在这里复用。
fn restore_panel(app: &AppHandle) {
    hide_icon(app);
    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 两项托盘菜单都是纯文本，不需要 Windows 为勾选标记预留的左侧列。
#[cfg(target_os = "windows")]
fn remove_menu_checkmark_space<R: tauri::Runtime>(
    menu: &Menu<R>,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::ContextMenu;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetMenuInfo, SetMenuInfo, HMENU, MENUINFO, MIM_STYLE, MNS_NOCHECK,
    };

    let handle = HMENU(menu.hpopupmenu()? as *mut std::ffi::c_void);
    let mut info = MENUINFO {
        cbSize: std::mem::size_of::<MENUINFO>() as u32,
        fMask: MIM_STYLE,
        ..Default::default()
    };
    // 句柄由仍存活的 Menu 持有；读取现有样式后只增加 NOCHECK，不覆盖其他标志。
    unsafe {
        GetMenuInfo(handle, &mut info)?;
        info.dwStyle |= MNS_NOCHECK;
        SetMenuInfo(handle, &info)?;
    }
    Ok(())
}

fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let settings_item = MenuItem::with_id(app, "open-settings", "设置", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 WordRay", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&settings_item, &quit_item])?;
    #[cfg(target_os = "windows")]
    if let Err(err) = remove_menu_checkmark_space(&menu) {
        println!("[WordRay] 无法取消托盘菜单的勾选占位：{err}");
    }
    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "WordRay 默认应用图标缺失")
    })?;

    TrayIconBuilder::with_id("main-tray")
        .tooltip("WordRay")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open-settings" => open_settings(app.clone()),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => restore_panel(tray.app_handle()),
            _ => {}
        })
        .build(app)?;

    Ok(())
}

// ---------------------------------------------------------------- 翻译

fn emit_error(app: &AppHandle, session: u64, message: impl Into<String>) {
    let _ = app.emit(
        "translation://error",
        ErrorPayload {
            session_id: session,
            message: message.into(),
        },
    );
}

fn begin_translation(app: &AppHandle, text: String) {
    // 取一份所有权：下面的 async 块要求捕获的东西是 'static
    let app = app.clone();

    let session = next_session();
    let _ = app.emit(
        "translation://start",
        StartPayload {
            session_id: session,
            source: text.clone(),
        },
    );

    let current = settings::load(&app);
    let Some(api_key) = current.api_key.clone() else {
        emit_error(
            &app,
            session,
            "还没有配置 DeepSeek API Key：点面板上的「设置」填一个即可",
        );
        return;
    };

    let config = deepseek::Config {
        api_key,
        base_url: current.base_url,
        model: current.model,
    };

    let app_for_delta = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = deepseek::stream_chat_split(
            &config,
            Some(SYSTEM_PROMPT),
            &text,
            Some(ALIGN_MARKER),
            |delta| {
                let _ = app_for_delta.emit(
                    "translation://delta",
                    DeltaPayload {
                        session_id: session,
                        text: delta.to_string(),
                    },
                );
            },
        )
        .await;

        match outcome {
            Ok(outcome) => {
                // 先把译文发出去：界面不该为了对齐表而多等
                let _ = app.emit(
                    "translation://done",
                    DonePayload {
                        session_id: session,
                        full_text: outcome.text.clone(),
                    },
                );

                // 完整译文到达即保存；后续对齐失败仍可回看这次翻译。
                let saved = history::record(&app, &text, &outcome.text);
                let _ = app.emit(
                    "history://changed",
                    HistoryChangedPayload { error: saved.error },
                );

                // 再解决对齐：内嵌的不合格就专门再要一次
                let mut pairs = parse_alignment(&outcome.tail).unwrap_or_default();
                let (mut usable, mut longest) = assess_alignment(&text, &outcome.text, &pairs);
                let mut origin = "内嵌";

                if !alignment_is_good(usable, longest, pairs.len()) {
                    println!(
                        "[WordRay] 内嵌对齐表不可用（可用 {usable}/{} 对，最长片段 {longest} 字），改用专门的对齐请求",
                        pairs.len()
                    );
                    match request_alignment(&config, &text, &outcome.text).await {
                        Ok(better) => {
                            let (better_usable, better_longest) =
                                assess_alignment(&text, &outcome.text, &better);
                            pairs = better;
                            usable = better_usable;
                            longest = better_longest;
                            origin = "专门请求";
                        }
                        Err(err) => {
                            println!("[WordRay] 专门的对齐请求也失败：{err}");
                        }
                    }
                }

                if usable > 0 {
                    println!(
                        "[WordRay] 对齐表：{usable} 对可用（最长 src 片段 {longest} 字，来源：{origin}）"
                    );
                } else {
                    println!("[WordRay] 对齐表不可用，选词时单独请求语义对齐");
                }

                // 无论成功与否都发一次：前端据此把"分析中"变成最终状态
                let _ = app.emit(
                    "translation://align",
                    AlignPayload {
                        session_id: session,
                        pairs: if usable > 0 { pairs } else { Vec::new() },
                    },
                );
            }
            Err(err) => emit_error(&app, session, err),
        }
    });
}

// ---------------------------------------------------------------- 命令

#[tauri::command]
fn get_state() -> StatePayload {
    StatePayload {
        hotkey: ACTIVE_HOTKEY.lock().ok().and_then(|guard| guard.clone()),
        panel_pinned: PANEL_PINNED.lock().map(|value| *value).unwrap_or(false),
        panel_wide: PANEL_WIDE.lock().map(|value| *value).unwrap_or(false),
    }
}

/// 手动切换面板形态（竖排 ↔ 左右分栏）。
///
/// **刻意不移动窗口**：用户可能刚把面板拖到顺手的位置，切布局不该把它挪走。
/// 但尺寸变了之后可能有一部分跑到屏幕外，所以要夹一次。
#[tauri::command]
fn toggle_panel_layout(app: AppHandle) {
    let wide = !PANEL_WIDE.lock().map(|value| *value).unwrap_or(false);

    let probe = app
        .get_webview_window("panel")
        .and_then(|win| win.outer_position().ok())
        .map(|position| (position.x, position.y))
        .unwrap_or((0, 0));

    let (width, height) = apply_panel_size(&app, wide, probe);

    if let Some(win) = app.get_webview_window("panel") {
        if let Ok(position) = win.outer_position() {
            let (x, y) = clamp_to_monitor(&app, position.x, position.y, width, height);
            let _ = win.set_position(PhysicalPosition::new(x, y));
        }
    }

    println!(
        "[WordRay] 面板切换为{}",
        if wide { "左右分栏" } else { "竖排" }
    );
}

/// 用户拖完面板：把当前位置固定下来，后续翻译不再自动挪动它
#[tauri::command]
fn pin_panel() {
    if let Ok(mut guard) = PANEL_PINNED.lock() {
        *guard = true;
    }
    println!("[WordRay] 面板位置已固定，后续翻译不再自动移动");
}

/// 取消固定：面板恢复"跟着选区走"
#[tauri::command]
fn unpin_panel() {
    if let Ok(mut guard) = PANEL_PINNED.lock() {
        *guard = false;
    }
    println!("[WordRay] 面板恢复跟随选区");
}

// ---------------------------------------------------------------- 设置

#[derive(Clone, Serialize)]
struct SettingsPayload {
    base_url: String,
    model: String,
    /// **只有设置窗口**能拿到明文密钥；面板与图标窗没有理由看到它
    api_key: Option<String>,
    key_source: settings::KeySource,
    config_path: String,
    default_base_url: String,
    default_model: String,
}

#[tauri::command]
fn get_settings(window: tauri::WebviewWindow, app: AppHandle) -> SettingsPayload {
    let current = settings::load(&app);
    let is_settings_window = window.label() == "settings";

    SettingsPayload {
        base_url: current.base_url,
        model: current.model,
        api_key: if is_settings_window {
            current.api_key
        } else {
            None
        },
        key_source: current.key_source,
        config_path: settings::describe_path(&app),
        default_base_url: settings::DEFAULT_BASE_URL.to_string(),
        default_model: settings::DEFAULT_MODEL.to_string(),
    }
}

/// 用一个结构体接收参数，避免 Tauri 命令参数"camelCase 还是 snake_case"的歧义：
/// serde 按字段名反序列化，前端传什么键就是什么键，不依赖框架的改名规则。
#[derive(Clone, serde::Deserialize)]
struct EndpointArgs {
    base_url: String,
    model: String,
    api_key: String,
}

/// 保存设置。`api_key` 传空字符串表示清除已保存的密钥。
#[tauri::command]
fn save_settings(app: AppHandle, args: EndpointArgs) -> Result<(), String> {
    settings::save(&app, &args.base_url, &args.model, Some(&args.api_key))
}

/// 用当前填写的参数真跑一次请求，验证 Key / 地址 / 模型是否可用。
///
/// 刻意走**真实的流式对话接口**而不是 `/models`：这样验的就是实际使用的那条路，
/// 地址写错、模型名写错都会在这里暴露。
#[tauri::command]
async fn test_settings(args: EndpointArgs) -> Result<String, String> {
    let api_key = args.api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("请先填写 API Key".to_string());
    }

    let config = deepseek::Config {
        api_key,
        base_url: args.base_url.trim().to_string(),
        model: args.model.trim().to_string(),
    };

    let reply = deepseek::stream_chat(&config, None, "ping", |_| {}).await?;
    Ok(format!(
        "连接成功，模型 {} 返回了 {} 个字符",
        config.model,
        reply.chars().count()
    ))
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

// ---------------------------------------------------------------- 历史

#[tauri::command]
fn get_history(app: AppHandle) -> history::HistorySnapshot {
    let snapshot = history::snapshot(&app);
    // 状态通知不会触发列表再次读取，重试成功后也能清掉面板上的保存失败提示。
    let _ = app.emit(
        "history://status",
        HistoryChangedPayload {
            error: snapshot.error.clone(),
        },
    );
    snapshot
}

#[tauri::command]
fn open_history(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("history")
        .ok_or_else(|| "历史窗口尚未初始化".to_string())?;
    window.unminimize().map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    // 窗口隐藏期间仍可能有翻译完成，每次打开都重新读取最新记录。
    window
        .emit("history://refresh", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn restore_history(app: AppHandle, id: String) -> Result<(), String> {
    // 只接受已保存的 ID，不信任前端自行提交的原文或译文。
    let entry = history::find(&app, &id)?;
    let session = next_session();
    hide_icon(&app);
    show_panel(&app, None, wants_wide_layout(&entry.source));
    app.emit(
        "translation://start",
        StartPayload {
            session_id: session,
            source: entry.source,
        },
    )
    .map_err(|error| error.to_string())?;
    app.emit(
        "translation://done",
        DonePayload {
            session_id: session,
            full_text: entry.translation,
        },
    )
    .map_err(|error| error.to_string())?;
    app.emit(
        "translation://align",
        AlignPayload {
            session_id: session,
            pairs: Vec::new(),
        },
    )
    .map_err(|error| error.to_string())?;
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.unminimize();
        let _ = panel.show();
        let _ = panel.set_focus();
    }
    if let Some(window) = app.get_webview_window("history") {
        let _ = window.hide();
    }
    Ok(())
}

/// 图标被点击：用之前识别好的选区开翻译（此时不再碰剪贴板）
#[tauri::command]
fn open_panel(app: AppHandle) {
    thread::spawn(move || {
        hide_icon(&app);

        let pending = PENDING.lock().ok().and_then(|mut guard| guard.take());

        match pending {
            Some(selection) => {
                let wide = wants_wide_layout(&selection.text);
                show_panel(&app, selection.rect, wide);
                begin_translation(&app, selection.text);
            }
            None => {
                let session = next_session();
                show_panel(&app, None, false);
                emit_error(&app, session, "没有识别到选中的文字，请重新划词");
            }
        }
    });
}

#[tauri::command]
fn dismiss_icon(app: AppHandle) {
    hide_icon(&app);
    if let Ok(mut guard) = PENDING.lock() {
        *guard = None;
    }
}

#[tauri::command]
fn close_panel(app: AppHandle) {
    if let Some(win) = app.get_webview_window("panel") {
        let _ = win.hide();
    }
}

#[tauri::command]
fn copy_result(text: String) -> Result<(), String> {
    clipboard::write_text(&text)
}

// ---------------------------------------------------------------- 划词

/// 已经证明"UIA 能读到选区"的前台窗口类。
///
/// 对这类应用**绝不**退到剪贴板注入：浏览器里拖一个链接也会被 Ctrl+C 复制成文本，
/// 从而冒出莫名其妙的翻译图标。UIA 读不到就当作没选中，保持克制。
/// 这个缓存只记正面结论，不记负面——Chromium 的 UIA 树要"被问到才建"，
/// 早期失败不代表这个应用永远读不到。
static UIA_CAPABLE_CLASSES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn is_uia_capable(class: &str) -> bool {
    if class.is_empty() {
        return false;
    }
    UIA_CAPABLE_CLASSES
        .lock()
        .map(|list| list.iter().any(|c| c.eq_ignore_ascii_case(class)))
        .unwrap_or(false)
}

fn remember_uia_capable(class: &str) {
    if class.is_empty() {
        return;
    }
    if let Ok(mut list) = UIA_CAPABLE_CLASSES.lock() {
        if !list.iter().any(|c| c.eq_ignore_ascii_case(class)) {
            println!("[WordRay] 记住这类应用支持 UIA 取词：{class}");
            list.push(class.to_string());
        }
    }
}

/// 记下选区并把图标挂到锚点旁边。
///
/// 锚点优先用 UIA 给出的选区矩形右边缘（这才是真正的"文本右侧"）；
/// 拿不到矩形时退到手势点（拖选通常在文字右端松手，位置也基本对）。
fn publish_selection(
    app: &AppHandle,
    text: String,
    rect: Option<uia::ScreenRect>,
    anchor_hint: (i32, i32),
    how: &str,
) {
    let (anchor_x, anchor_y) = match rect {
        Some((left, top, width, height)) => ((left + width) as i32, (top + height / 2.0) as i32),
        None => anchor_hint,
    };

    println!(
        "[WordRay] {how} 得到选区：{} 字符，锚点 ({anchor_x}, {anchor_y})，来源={}",
        text.chars().count(),
        if rect.is_some() {
            "UIA 矩形"
        } else {
            "手势点"
        }
    );

    if let Ok(mut guard) = PENDING.lock() {
        *guard = Some(Selection { text, rect });
    }
    show_icon(app, anchor_x, anchor_y);
}

/// 选区分发：优先 UIA（不碰剪贴板），读不到再按情况退到剪贴板核实。
fn handle_selection(
    app: &AppHandle,
    probe: (i32, i32),
    anchor_hint: (i32, i32),
    points: &[(i32, i32)],
    how: &str,
) {
    if points.iter().any(|&p| {
        point_in_window(app, "icon", p)
            || point_in_window(app, "panel", p)
            || point_in_window(app, "settings", p)
            || point_in_window(app, "history", p)
    }) {
        return;
    }

    let class = clipboard::foreground_class_name().unwrap_or_default();

    // 1) 主路径：UI Automation。全程不碰剪贴板。
    if let Some(selection) = uia::read_selection_at(points) {
        match selection.rect {
            Some(rect) => {
                // 关键守卫：手势起点必须落在选区附近。
                // 否则多半是焦点元素上残留的**旧选区**（用户其实在拖窗口或拖滚动条）。
                if !gesture::point_near_rect(probe, rect) {
                    println!(
                        "[WordRay] {how} 手势起点 ({}, {}) 不在选区附近，判定为旧选区，忽略",
                        probe.0, probe.1
                    );
                    return;
                }
                remember_uia_capable(&class);
                publish_selection(app, selection.text, Some(rect), anchor_hint, how);
            }
            None => {
                // 有文字但拿不到矩形：仍然可用，只是锚点退到手势点
                remember_uia_capable(&class);
                publish_selection(app, selection.text, None, anchor_hint, how);
            }
        }
        return;
    }

    // 2) UIA 读不到。如果这类应用此前证明过 UIA 可用，就不要退到剪贴板——
    //    否则浏览器里拖链接、拖图片都会变成一次"复制 → 冒图标"。
    if is_uia_capable(&class) {
        println!("[WordRay] {how} UIA 读不到选区，而该类应用支持 UIA，判定为未选中文字");
        return;
    }

    // 3) 退到剪贴板核实：注入一次 Ctrl+C，看剪贴板的写入序号有没有变。
    //    这是微信这类不暴露 UIA 文本的应用唯一的取词途径。
    match clipboard::capture_selection() {
        Ok(text) => publish_selection(app, text, None, anchor_hint, how),
        Err(err) => {
            println!("[WordRay] {how} UIA 与剪贴板都没取到选区：{err}");
        }
    }
}

fn start_gesture_worker(app: AppHandle) -> mpsc::Sender<RawEvent> {
    let (sender, receiver) = mpsc::channel::<RawEvent>();

    thread::spawn(move || {
        uia::init();
        let mut detector = GestureDetector::new();

        for event in receiver {
            let Some(gesture) = detector.feed(event) else {
                continue;
            };

            match gesture {
                Gesture::DragSelect { start, end } => {
                    // 起手点用来核对"这次手势是不是真在选文字"；
                    // 松手点既用于查询，也当图标锚点（拖选通常在文字右端松手）
                    handle_selection(&app, start, end, &[end, start], "拖选");
                }
                Gesture::DoubleClick { at } => {
                    handle_selection(&app, at, at, &[at], "双击");
                }
                Gesture::Click { at } => {
                    // 点在图标上时不能收：那一下要留给 WebView 处理点击
                    if !point_in_window(&app, "icon", at) {
                        hide_icon(&app);
                    }
                }
            }
        }
    });

    sender
}

// ---------------------------------------------------------------- 热键（备选通道）

/// 候选热键链，第一个注册成功的生效。
///
/// 全局热键是独占资源，别的程序注册了我们就注册不上。若只有一个键且失败即退出，
/// 用户看到的是"程序一闪就没"，无从判断原因。实测本机 `Ctrl+Alt+T` 已被占用。
fn candidate_shortcuts() -> Vec<(Modifiers, Code)> {
    vec![
        (Modifiers::CONTROL | Modifiers::ALT, Code::KeyT),
        (Modifiers::ALT | Modifiers::SHIFT, Code::KeyT),
        (Modifiers::CONTROL | Modifiers::ALT, Code::KeyY),
        (Modifiers::ALT | Modifiers::SHIFT, Code::KeyY),
        (
            Modifiers::CONTROL | Modifiers::SHIFT | Modifiers::ALT,
            Code::KeyD,
        ),
    ]
}

fn shortcut_label(modifiers: Modifiers, code: Code) -> String {
    let mut parts: Vec<String> = Vec::new();
    if modifiers.contains(Modifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if modifiers.contains(Modifiers::ALT) {
        parts.push("Alt".to_string());
    }
    let key = format!("{code:?}");
    parts.push(key.trim_start_matches("Key").to_string());
    parts.join(" + ")
}

/// 热键通道：给 UIA 读不到选区的应用留一条兜底（走剪贴板注入）。
fn handle_hotkey(app: &AppHandle) {
    let app = app.clone();

    thread::spawn(move || {
        hide_icon(&app);

        let text = match clipboard::capture_selection() {
            Ok(text) => text,
            Err(err) => {
                let session = next_session();
                show_panel(&app, None, false);
                emit_error(&app, session, err.to_string());
                return;
            }
        };

        // 热键取到的文字同样可能很长，走同一套形态判断
        let wide = wants_wide_layout(&text);
        show_panel(&app, None, wide);
        begin_translation(&app, text);
    });
}

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        handle_hotkey(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_state,
            selection_alignment::align_selection,
            open_panel,
            dismiss_icon,
            close_panel,
            copy_result,
            pin_panel,
            unpin_panel,
            toggle_panel_layout,
            get_settings,
            save_settings,
            test_settings,
            open_settings,
            get_history,
            open_history,
            restore_history
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if matches!(window.label(), "panel" | "settings" | "history") {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();

            // 0) 图标窗：尽早剥掉系统窗口样式，否则它会被夹到 136×39
            if let Some(icon) = app.get_webview_window("icon") {
                make_icon_window_small(&icon);
            }

            // 托盘成功注册后再启动全局监听，确保窗口隐藏后始终有恢复/退出入口。
            setup_tray(app)?;

            // 1) 全局鼠标钩子：自己一个线程，内部跑消息循环
            let sender = start_gesture_worker(handle.clone());
            thread::spawn(move || {
                if let Err(err) = mouse_hook::run(sender) {
                    println!("[WordRay] {err}");
                }
            });

            // 2) 全局热键：候选链里第一个能注册上的
            let mut active: Option<String> = None;
            for (modifiers, code) in candidate_shortcuts() {
                let shortcut = Shortcut::new(Some(modifiers), code);
                match app.global_shortcut().register(shortcut) {
                    Ok(()) => {
                        let label = shortcut_label(modifiers, code);
                        println!("[WordRay] 已注册全局热键：{label}");
                        active = Some(label);
                        break;
                    }
                    Err(err) => {
                        println!("[WordRay] 热键 {code:?} 注册失败：{err}");
                    }
                }
            }
            if active.is_none() {
                println!("[WordRay] 警告：候选热键全部被占用，仅能靠划词触发");
            }
            if let Ok(mut guard) = ACTIVE_HOTKEY.lock() {
                *guard = active;
            }

            // 3) 首次运行：还没有配置 Key 就直接把设置窗打开。
            //    否则用户按热键只会看到一句"没有配置"，不知道该去哪里配。
            if settings::load(&handle).api_key.is_none() {
                println!("[WordRay] 尚未配置 API Key，自动打开设置窗口");
                if let Some(win) = handle.get_webview_window("settings") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run tauri application");
}

#[cfg(test)]
mod alignment_tests {
    use super::*;

    fn pair(src: &str, dst: &str) -> AlignPair {
        AlignPair {
            src: src.to_string(),
            dst: dst.to_string(),
        }
    }

    #[test]
    fn assess_counts_usable_pairs_and_longest_src() {
        let (usable, longest) = assess_alignment(
            "这个功能以后会加上。",
            "This feature will be added later.",
            &[
                pair("这个功能", "This feature"),
                pair("以后", "later"),
                pair("会加上", "will be added"),
            ],
        );
        assert_eq!(usable, 3);
        assert_eq!(longest, 4); // 「这个功能」
    }

    #[test]
    fn out_of_order_dst_is_still_counted() {
        // 「再说」对应更靠前的 "talk"：dst 必须允许回退到全局查找
        let (usable, _) = assess_alignment(
            "我们以后再说",
            "Let's talk later",
            &[
                pair("我们", "Let's"),
                pair("以后", "later"),
                pair("再说", "talk"),
            ],
        );
        assert_eq!(usable, 3);
    }

    #[test]
    fn unlocatable_pairs_are_not_counted() {
        let (usable, _) = assess_alignment(
            "你好世界",
            "Hello world",
            &[
                pair("你好", "Hello"),
                pair("不存在", "nope"),
                pair("世界", "world"),
            ],
        );
        assert_eq!(usable, 2);
    }

    #[test]
    fn coarse_or_unusable_alignment_is_rejected() {
        assert!(!alignment_is_good(1, 30, 1), "粒度太粗应被拒绝");
        assert!(!alignment_is_good(0, 4, 3), "一对都定位不到应被拒绝");
        assert!(!alignment_is_good(1, 4, 10), "只有 10% 能定位应被拒绝");
        assert!(alignment_is_good(2, 4, 3), "2/3 可用且粒度够细应通过");
    }

    #[test]
    fn parse_tolerates_code_fences_and_prose() {
        let text = "好的，如下：\n```json\n[{\"src\":\"甲\",\"dst\":\"A\"}]\n```\n希望有帮助";
        let pairs = parse_alignment(text).expect("应当能从噪音里抠出来");
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_alignment("没有数组").is_none());
        assert!(parse_alignment("[]").is_none());
        assert!(parse_alignment("[{\"x\":1}]").is_none());
    }
}

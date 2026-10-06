//! 用 UI Automation 读取"当前选区"的文字与屏幕矩形。
//!
//! 走这条路最大的收益：**完全不碰剪贴板**，也不需要目标应用保持前台。
//!
//! 已知边界（阶段 3 处理）：不暴露 UIA 文本的应用（WPS 的 Qt 版这类）
//! 读不到选区；此时我们选择"什么都不做"，而不是退到剪贴板注入——
//! 因为 UIA 读不到时无法区分"真划词"和"拖窗口/拖滚动条"，
//! 贸然退到剪贴板会让每次拖窗口都冒出一个图标。

use windows::core::BSTR;
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextRange, UIA_TextPatternId,
};

/// 屏幕坐标下的矩形：left, top, width, height
pub type ScreenRect = (f64, f64, f64, f64);

#[derive(Debug, Clone)]
pub struct Selection {
    pub text: String,
    pub rect: Option<ScreenRect>,
}

/// 在将要调用 UIA 的线程上先调一次。重复调用安全（第二次返回 S_FALSE，忽略即可）。
pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// 依次在给定的屏幕坐标上找带 TextPattern 的元素并读其选区。
///
/// 会尝试多个点，是因为拖选常常在文字**外面**松手（往后拖过行尾），
/// 那个位置 `ElementFromPoint` 命中的可能根本不是文本控件。
/// 全部失败后还会退到"当前焦点元素"——划词结束时焦点仍在原应用上。
pub fn read_selection_at(points: &[(i32, i32)]) -> Option<Selection> {
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;

        // 沿祖先链往上找必须用 TreeWalker：IUIAutomationElement 自身没有 GetParentElement
        let walker = automation.ControlViewWalker().ok()?;

        for &(x, y) in points {
            if let Ok(element) = automation.ElementFromPoint(POINT { x, y }) {
                if let Some(selection) = walk_up(&walker, &element) {
                    return Some(selection);
                }
            }
        }

        if let Ok(focused) = automation.GetFocusedElement() {
            if let Some(selection) = walk_up(&walker, &focused) {
                return Some(selection);
            }
        }
    }

    None
}

/// 命中的常常只是外壳（浏览器里尤其明显），沿祖先链往上找 TextPattern。
unsafe fn walk_up(
    walker: &windows::Win32::UI::Accessibility::IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
) -> Option<Selection> {
    let mut current = element.clone();

    for _ in 0..12 {
        if let Ok(pattern) =
            current.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        {
            if let Some(selection) = selection_from_pattern(&pattern) {
                return Some(selection);
            }
        }

        match walker.GetParentElement(&current) {
            Ok(parent) => current = parent,
            Err(_) => break,
        }
    }

    None
}

unsafe fn selection_from_pattern(pattern: &IUIAutomationTextPattern) -> Option<Selection> {
    let ranges = pattern.GetSelection().ok()?;
    if ranges.Length().unwrap_or(0) <= 0 {
        return None;
    }

    let range = ranges.GetElement(0).ok()?;
    let text: BSTR = range.GetText(-1).ok()?;
    let text = text.to_string();

    // 空选区（只有插入符）必须当作"没有选中"——否则单击也会冒图标
    if text.trim().is_empty() {
        return None;
    }

    Some(Selection {
        text,
        rect: bounding_rect(&range),
    })
}

/// `GetBoundingRectangles` 返回 SAFEARRAY(double)，每 4 个一组。
/// 按官方文档解释为 left, top, width, height；多行选区取并集。
unsafe fn bounding_rect(range: &IUIAutomationTextRange) -> Option<ScreenRect> {
    let array = range.GetBoundingRectangles().ok()?;
    if array.is_null() {
        return None;
    }

    let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
    if SafeArrayAccessData(array, &mut data).is_err() {
        return None;
    }

    // 这两个函数在 windows-rs 里返回 Result<i32>，不是出参
    let lower = SafeArrayGetLBound(array, 1).unwrap_or(0);
    let upper = SafeArrayGetUBound(array, 1).unwrap_or(-1);
    let count = (upper - lower + 1).max(0) as usize;

    let values = std::slice::from_raw_parts(data as *const f64, count);
    let mut union: Option<ScreenRect> = None;
    for chunk in values.chunks_exact(4) {
        let (left, top, width, height) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        union = Some(match union {
            None => (left, top, width, height),
            Some((pl, pt, pw, ph)) => {
                let min_left = pl.min(left);
                let min_top = pt.min(top);
                let max_right = (pl + pw).max(left + width);
                let max_bottom = (pt + ph).max(top + height);
                (
                    min_left,
                    min_top,
                    max_right - min_left,
                    max_bottom - min_top,
                )
            }
        });
    }

    let _ = SafeArrayUnaccessData(array);
    let _ = SafeArrayDestroy(array);
    union
}

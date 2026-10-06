//! 纯手势状态机：把原始的鼠标按下/松开事件判成"拖选"或"双击选词"。
//!
//! 刻意做成不依赖任何 Win32 / Tauri 类型的纯逻辑，这样可以直接单测。
//! 只吃 Down / Up 两类事件：位移用"按下点与松开点之间的距离"算，
//! 因此不需要处理高频的 Move 事件。

/// 判定为"拖选"的最小位移（像素）
pub const DRAG_MIN_PX: i32 = 5;
/// 双击的时间窗（毫秒）
pub const DOUBLE_CLICK_MS: u64 = 420;
/// 双击允许的位置偏差（像素）
pub const DOUBLE_CLICK_DIST: i32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawKind {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: RawKind,
    pub x: i32,
    pub y: i32,
    pub t_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// 拖选：左键按下到松开位移达到阈值
    DragSelect { start: (i32, i32), end: (i32, i32) },
    /// 双击选词
    DoubleClick { at: (i32, i32) },
    /// 单击（位移不足阈值）
    Click { at: (i32, i32) },
}

/// 判断某个屏幕点是否落在选区矩形附近（含余量）。
///
/// 用途：划词结束后读到的是"当前选区"，但**焦点元素上可能还挂着上一次的旧选区**
/// ——用户拖一个窗口、拖滚动条时，UIA 照样会返回一个非空选区。
/// 用手势的起点去核对矩形，就能把这些假阳性挡掉。
///
/// 余量取 `max(8, 矩形高度)`：拖选的起手点会吸附到字符边界，
/// 字符有多宽误差就能有多大，写死 8px 会把正常划词误杀。
pub fn point_near_rect(point: (i32, i32), rect: (f64, f64, f64, f64)) -> bool {
    let (left, top, width, height) = rect;
    let margin = 8.0_f64.max(height);
    let x = point.0 as f64;
    let y = point.1 as f64;

    x >= left - margin
        && x <= left + width + margin
        && y >= top - margin
        && y <= top + height + margin
}

#[derive(Default)]
pub struct GestureDetector {
    down: Option<(i32, i32)>,
    prev_down: Option<((i32, i32), u64)>,
    double_pending: bool,
}

impl GestureDetector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, event: RawEvent) -> Option<Gesture> {
        match event.kind {
            RawKind::Down => {
                let at = (event.x, event.y);
                // 与"上一次按下"比较：时间与位置都在窗口内才算双击的第二下
                self.double_pending = matches!(
                    self.prev_down,
                    Some((prev, t))
                        if event.t_ms.saturating_sub(t) <= DOUBLE_CLICK_MS
                            && (prev.0 - event.x).abs() <= DOUBLE_CLICK_DIST
                            && (prev.1 - event.y).abs() <= DOUBLE_CLICK_DIST
                );
                self.prev_down = Some((at, event.t_ms));
                self.down = Some(at);
                None
            }
            RawKind::Up => {
                let start = self.down.take()?;

                if self.double_pending {
                    self.double_pending = false;
                    // 清掉 prev_down，避免三连击被继续判成双击
                    self.prev_down = None;
                    return Some(Gesture::DoubleClick {
                        at: (event.x, event.y),
                    });
                }

                let dx = (event.x - start.0).abs();
                let dy = (event.y - start.1).abs();
                if dx.max(dy) >= DRAG_MIN_PX {
                    Some(Gesture::DragSelect {
                        start,
                        end: (event.x, event.y),
                    })
                } else {
                    Some(Gesture::Click {
                        at: (event.x, event.y),
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn down(x: i32, y: i32, t: u64) -> RawEvent {
        RawEvent {
            kind: RawKind::Down,
            x,
            y,
            t_ms: t,
        }
    }

    fn up(x: i32, y: i32, t: u64) -> RawEvent {
        RawEvent {
            kind: RawKind::Up,
            x,
            y,
            t_ms: t,
        }
    }

    #[test]
    fn drag_beyond_threshold_is_drag_select() {
        let mut d = GestureDetector::new();
        assert_eq!(d.feed(down(100, 100, 0)), None);
        assert_eq!(
            d.feed(up(180, 104, 200)),
            Some(Gesture::DragSelect {
                start: (100, 100),
                end: (180, 104)
            })
        );
    }

    #[test]
    fn tiny_movement_is_click() {
        let mut d = GestureDetector::new();
        assert_eq!(d.feed(down(100, 100, 0)), None);
        assert_eq!(
            d.feed(up(102, 101, 80)),
            Some(Gesture::Click { at: (102, 101) })
        );
    }

    #[test]
    fn two_quick_clicks_are_double_click() {
        let mut d = GestureDetector::new();
        d.feed(down(100, 100, 0));
        assert_eq!(
            d.feed(up(100, 100, 50)),
            Some(Gesture::Click { at: (100, 100) })
        );
        d.feed(down(101, 100, 150));
        assert_eq!(
            d.feed(up(101, 100, 200)),
            Some(Gesture::DoubleClick { at: (101, 100) })
        );
    }

    #[test]
    fn slow_second_click_is_not_double_click() {
        let mut d = GestureDetector::new();
        d.feed(down(100, 100, 0));
        d.feed(up(100, 100, 50));
        d.feed(down(100, 100, 2000));
        assert_eq!(
            d.feed(up(100, 100, 2050)),
            Some(Gesture::Click { at: (100, 100) })
        );
    }

    #[test]
    fn far_apart_quick_clicks_are_not_double_click() {
        let mut d = GestureDetector::new();
        d.feed(down(100, 100, 0));
        d.feed(up(100, 100, 50));
        d.feed(down(300, 100, 150));
        assert_eq!(
            d.feed(up(300, 100, 200)),
            Some(Gesture::Click { at: (300, 100) })
        );
    }

    #[test]
    fn triple_click_does_not_produce_a_second_double_click() {
        let mut d = GestureDetector::new();
        d.feed(down(100, 100, 0));
        d.feed(up(100, 100, 40));
        d.feed(down(100, 100, 90));
        assert!(matches!(
            d.feed(up(100, 100, 130)),
            Some(Gesture::DoubleClick { .. })
        ));
        d.feed(down(100, 100, 180));
        assert_eq!(
            d.feed(up(100, 100, 220)),
            Some(Gesture::Click { at: (100, 100) })
        );
    }

    #[test]
    fn point_inside_selection_is_near() {
        // 选区 100..300 横向，200..220 纵向
        assert!(point_near_rect((150, 210), (100.0, 200.0, 200.0, 20.0)));
    }

    #[test]
    fn point_at_selection_edge_is_still_near() {
        // 起手点会吸附到字符边界，行高 20 → 余量 20
        assert!(point_near_rect((100, 200), (100.0, 200.0, 200.0, 20.0)));
        assert!(point_near_rect((300, 220), (100.0, 200.0, 200.0, 20.0)));
    }

    #[test]
    fn point_far_away_is_not_near() {
        // 这是关键用例：拖窗口标题栏时读到的旧选区离手势很远，必须判否
        assert!(!point_near_rect((1200, 40), (100.0, 400.0, 200.0, 20.0)));
    }
}

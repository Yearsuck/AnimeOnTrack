//! Window geometry fitting to keep app windows within monitor work areas.
//!
//! Windows placed at default coordinates or sized larger than the available
//! workspace (due to display scaling, taskbars, or smaller resolutions) can
//! end up partially behind the Windows taskbar or off-screen. This module
//! provides pure coordinate math to clamp and center windows within a
//! monitor's work area, plus a helper to apply those adjustments to a
//! Tauri `WebviewWindow`.

/// (x, y, width, height) in physical pixels.
pub type Rect = (i32, i32, u32, u32);

/// Fits a window rect entirely inside a work area rect.
///
/// Returns `None` when `win` already lies entirely inside `work`.
/// Otherwise returns `Some(Rect)` that:
/// - shrinks width and height to at most the work area's size (clamped to at least 1)
/// - shifts `x` and `y` so the window lies within `work`, preserving the original
///   position whenever possible without re-centering:
///   `x = win.x.clamp(work.x, work.x + work.w - new_w)`
///   `y = win.y.clamp(work.y, work.y + work.h - new_h)`
pub fn fit_into(win: Rect, work: Rect) -> Option<Rect> {
    let (win_x, win_y, win_w, win_h) = win;
    let (work_x, work_y, work_w, work_h) = work;

    // A window of zero or negative dimension is invalid and cannot be considered inside.
    let is_inside = win_w > 0
        && win_h > 0
        && win_w <= work_w
        && win_h <= work_h
        && win_x >= work_x
        && (win_x as i64 + win_w as i64) <= (work_x as i64 + work_w as i64)
        && win_y >= work_y
        && (win_y as i64 + win_h as i64) <= (work_y as i64 + work_h as i64);

    if is_inside {
        return None;
    }

    // Shrink size to at most the work area's size, keeping at least 1px.
    let new_w = win_w.min(work_w).max(1);
    let new_h = win_h.min(work_h).max(1);

    // Compute maximum allowable position to keep the right/bottom edges inside the work area.
    let max_x = work_x + (work_w.saturating_sub(new_w)) as i32;
    let max_y = work_y + (work_h.saturating_sub(new_h)) as i32;

    let new_x = win_x.clamp(work_x, max_x.max(work_x));
    let new_y = win_y.clamp(work_y, max_y.max(work_y));

    Some((new_x, new_y, new_w, new_h))
}

/// Clamps size to the work area and centres it inside the work area.
pub fn centered_in(size: (u32, u32), work: Rect) -> Rect {
    let (w, h) = size;
    let (work_x, work_y, work_w, work_h) = work;

    let new_w = w.min(work_w).max(1);
    let new_h = h.min(work_h).max(1);

    let offset_x = (work_w.saturating_sub(new_w) / 2) as i32;
    let offset_y = (work_h.saturating_sub(new_h) / 2) as i32;

    (work_x + offset_x, work_y + offset_y, new_w, new_h)
}

/// Fits a window inside its current monitor's work area (excluding taskbars,
/// docks, etc.), shrinking its size if necessary and shifting its position
/// so no part of the window sits off-screen or behind the taskbar.
///
/// Best-effort: ignores errors, never panics, does nothing if the window is
/// maximized or if monitor geometry cannot be queried.
pub fn fit_window_to_work_area(win: &tauri::WebviewWindow) {
    if win.is_maximized().unwrap_or(false) {
        return;
    }
    let (pos, size, monitor) = match (win.outer_position(), win.outer_size(), win.current_monitor()) {
        (Ok(p), Ok(s), Ok(Some(m))) => (p, s, m),
        _ => return,
    };
    let work = monitor.work_area();
    let win_rect: Rect = (pos.x, pos.y, size.width, size.height);
    let work_rect: Rect = (
        work.position.x,
        work.position.y,
        work.size.width,
        work.size.height,
    );
    if let Some((x, y, w, h)) = fit_into(win_rect, work_rect) {
        // Set size first before position so the window manager does not clamp
        // the new position against outdated window bounds.
        set_outer_size(win, size, (w, h));
        let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)));
    }
}

/// `set_size` sets the INNER (client) size, while the fitting math works on
/// the outer frame (title bar, borders, the invisible resize border of a
/// frameless window). Subtract the frame so the outer size lands on `target`;
/// without this a decorated viewer ends up a title bar taller than the work area.
pub fn set_outer_size(win: &tauri::WebviewWindow, outer: tauri::PhysicalSize<u32>, target: (u32, u32)) {
    let (fw, fh) = match win.inner_size() {
        Ok(inner) => (
            outer.width.saturating_sub(inner.width),
            outer.height.saturating_sub(inner.height),
        ),
        Err(_) => (0, 0),
    };
    let w = target.0.saturating_sub(fw).max(1);
    let h = target.1.saturating_sub(fh).max(1);
    let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(w, h)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fit_into_taller_than_work_area() {
        // Taller than work area: height clamped to 1020, y moved up from 31 to 0.
        assert_eq!(
            fit_into((160, 31, 1600, 1040), (0, 0, 1920, 1020)),
            Some((160, 0, 1600, 1020))
        );
    }

    #[test]
    fn test_fit_into_already_fits() {
        // Fits entirely: 31 + 900 = 931 <= 1020.
        assert_eq!(
            fit_into((160, 31, 1600, 900), (0, 0, 1920, 1020)),
            None
        );
    }

    #[test]
    fn test_fit_into_bottom_exceeds_work_area() {
        // Bottom 200 + 900 = 1100 > 1020: moved up by 80, size kept.
        assert_eq!(
            fit_into((100, 200, 1600, 900), (0, 0, 1920, 1020)),
            Some((100, 120, 1600, 900))
        );
    }

    #[test]
    fn test_fit_into_second_monitor() {
        // Second monitor to the right: fits inside (1920..3200, 0..984).
        assert_eq!(
            fit_into((2000, 50, 800, 600), (1920, 0, 1280, 984)),
            None
        );
    }

    #[test]
    fn test_fit_into_wider_and_taller_than_work_area() {
        // Window wider and taller than 1366x728 work area: clamped to work area.
        assert_eq!(
            fit_into((0, 0, 1600, 1040), (0, 0, 1366, 728)),
            Some((0, 0, 1366, 728))
        );
    }

    #[test]
    fn test_fit_into_taskbar_on_top() {
        // Taskbar on top (work area starts at y=48): moved down to y=48.
        assert_eq!(
            fit_into((10, 0, 800, 600), (0, 48, 1920, 1032)),
            Some((10, 48, 800, 600))
        );
    }

    #[test]
    fn test_centered_in_clamps_and_centers() {
        assert_eq!(
            centered_in((1600, 1040), (0, 0, 1920, 1020)),
            (160, 0, 1600, 1020)
        );
    }

    #[test]
    fn test_centered_in_offset_monitor() {
        assert_eq!(
            centered_in((1600, 1000), (1920, 0, 1920, 1032)),
            (2080, 16, 1600, 1000)
        );
    }

    #[test]
    fn test_property_fit_into_and_centered_in_bounds() {
        let work_areas = [
            (0, 0, 1920, 1080),
            (0, 0, 1920, 1020),
            (0, 48, 1920, 1032),
            (1920, 0, 1280, 984),
            (-1920, 0, 1920, 1040),
            (0, 0, 1366, 728),
            (100, 100, 800, 600),
        ];

        let windows = [
            (-500, -200, 800, 600),
            (0, 0, 1280, 832),
            (160, 31, 1600, 1040),
            (100, 200, 1600, 900),
            (2500, 1500, 3000, 2000),
            (0, 0, 1600, 1040),
            (10, 0, 800, 600),
            (50, 50, 100, 100),
            (0, 0, 0, 0),
        ];

        for &work in &work_areas {
            let (work_x, work_y, work_w, work_h) = work;
            let work_right = work_x as i64 + work_w as i64;
            let work_bottom = work_y as i64 + work_h as i64;

            for &win in &windows {
                if let Some(r) = fit_into(win, work) {
                    assert!(r.2 > 0, "fit_into width must be > 0, got {}", r.2);
                    assert!(r.3 > 0, "fit_into height must be > 0, got {}", r.3);
                    assert!(
                        r.0 >= work_x,
                        "fit_into x ({}) must be >= work_x ({})",
                        r.0,
                        work_x
                    );
                    assert!(
                        r.1 >= work_y,
                        "fit_into y ({}) must be >= work_y ({})",
                        r.1,
                        work_y
                    );
                    let r_right = r.0 as i64 + r.2 as i64;
                    let r_bottom = r.1 as i64 + r.3 as i64;
                    assert!(
                        r_right <= work_right,
                        "fit_into right edge ({}) must be <= work_right ({}) for win {:?}, work {:?}",
                        r_right,
                        work_right,
                        win,
                        work
                    );
                    assert!(
                        r_bottom <= work_bottom,
                        "fit_into bottom edge ({}) must be <= work_bottom ({}) for win {:?}, work {:?}",
                        r_bottom,
                        work_bottom,
                        win,
                        work
                    );
                }

                let c = centered_in((win.2, win.3), work);
                assert!(c.2 > 0, "centered_in width must be > 0, got {}", c.2);
                assert!(c.3 > 0, "centered_in height must be > 0, got {}", c.3);
                assert!(
                    c.0 >= work_x,
                    "centered_in x ({}) must be >= work_x ({})",
                    c.0,
                    work_x
                );
                assert!(
                    c.1 >= work_y,
                    "centered_in y ({}) must be >= work_y ({})",
                    c.1,
                    work_y
                );
                let c_right = c.0 as i64 + c.2 as i64;
                let c_bottom = c.1 as i64 + c.3 as i64;
                assert!(
                    c_right <= work_right,
                    "centered_in right edge ({}) must be <= work_right ({}) for size {:?}, work {:?}",
                    c_right,
                    work_right,
                    (win.2, win.3),
                    work
                );
                assert!(
                    c_bottom <= work_bottom,
                    "centered_in bottom edge ({}) must be <= work_bottom ({}) for size {:?}, work {:?}",
                    c_bottom,
                    work_bottom,
                    (win.2, win.3),
                    work
                );
            }
        }
    }
}

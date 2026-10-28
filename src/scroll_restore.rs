//! Row arithmetic for the scroll position remembered per tab and history entry.
//!
//! Switching tabs restores the first visible row the tab was left at. The item
//! that was selected must not move that viewport: only an item outside the
//! restored rows is brought in, and then only to the nearest edge. Without this
//! the restored selection scrolled itself to the top of the window.

/// First visible row after restoring `saved` while keeping `focus_row` visible.
///
/// Returns `None` when the restored viewport already contains the item, so the
/// caller leaves the restored position untouched.
pub fn reveal_after_restore(saved: f32, focus_row: usize, visible_rows: f32) -> Option<f32> {
    let saved = saved.max(0.);
    let visible = visible_rows.max(1.);
    let focus = focus_row as f32;
    if focus < saved {
        // Above the viewport: put it at the top.
        Some(focus)
    } else if focus + 1. > saved + visible {
        // Below the viewport: put it at the bottom, keeping as much of the
        // restored position as possible.
        Some((focus - visible + 1.).max(0.))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_viewport_keeps_a_visible_selection_in_place() {
        // Row 20 is restored, the selected item is row 24 and 30 rows fit.
        assert_eq!(reveal_after_restore(20., 24, 30.), None);
        // The boundary rows count as visible too.
        assert_eq!(reveal_after_restore(20., 20, 30.), None);
        assert_eq!(reveal_after_restore(20., 49, 30.), None);
    }

    #[test]
    fn selection_above_the_restored_rows_moves_to_the_top() {
        assert_eq!(reveal_after_restore(20., 19, 30.), Some(19.));
        assert_eq!(reveal_after_restore(20., 0, 30.), Some(0.));
    }

    #[test]
    fn selection_below_the_restored_rows_moves_to_the_bottom() {
        assert_eq!(reveal_after_restore(20., 50, 30.), Some(21.));
        // Clamping keeps the first visible row at or after the start.
        assert_eq!(reveal_after_restore(0., 40, 10.), Some(31.));
        assert_eq!(reveal_after_restore(0., 0, 10.), None);
    }

    #[test]
    fn a_degenerate_viewport_still_uses_one_row() {
        assert_eq!(reveal_after_restore(0., 0, 0.), None);
        assert_eq!(reveal_after_restore(0., 1, 0.), Some(1.));
    }
}

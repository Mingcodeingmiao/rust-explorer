//! Content-view column arithmetic. Kept separate so the responsive rules can
//! be tested without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentColumns {
    /// Icon plus name/type block. Never wider than 160 logical pixels (280 physical pixels at 175% DPI).
    pub first: f32,
    /// Extra column with type and image resolution, only for wide windows.
    pub middle: Option<f32>,
    /// Date modified and size.
    pub right: Option<f32>,
}

// 280 physical pixels at 175% DPI. GPUI measures in logical pixels.
pub const FIRST_MAX: f32 = 160.;
pub const GAP: f32 = 16.;

pub fn columns(available: f32, has_image: bool) -> ContentColumns {
    let available = available.max(0.);
    let first = (available * 0.45).min(FIRST_MAX);
    let show_right = available >= if has_image { 480. } else { 360. };
    let right = (available - first - GAP).clamp(1., 240.);
    ContentColumns {
        first,
        middle: (has_image && show_right).then_some((available - first - right - GAP).max(1.)),
        right: show_right.then_some(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_column_grows_to_the_ceiling_then_stops() {
        assert!(columns(240., false).first < columns(300., false).first);
        assert_eq!(columns(900., false).first * 1.75, 280.);
        assert_eq!(columns(1400., false).first, FIRST_MAX);
    }
    #[test]
    fn narrow_windows_drop_the_secondary_columns() {
        let narrow = columns(280., false);
        assert!(narrow.first <= FIRST_MAX);
        assert_eq!(narrow.right, None);
        assert_eq!(narrow.middle, None);
    }
    #[test]
    fn image_middle_column_only_appears_when_there_is_room() {
        assert_eq!(columns(400., true).middle, None);
        let wide = columns(1000., true);
        assert_eq!(wide.first, FIRST_MAX);
        assert!(wide.middle.unwrap() > 0.);
        assert!(wide.right.unwrap() > 0.);
    }
}

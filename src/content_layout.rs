//! Content-view column arithmetic. Kept separate so the responsive rules can
//! be tested without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentColumns {
    /// Icon plus name/type block. Never wider than 280 logical pixels.
    pub first: f32,
    /// Extra column with type and image resolution, only for wide windows.
    pub middle: Option<f32>,
    /// Date modified and size.
    pub right: Option<f32>,
}

pub const FIRST_MAX: f32 = 280.;
pub const FIRST_MIN: f32 = 180.;
pub const MIDDLE_WIDTH: f32 = 180.;
pub const RIGHT_MIN: f32 = 200.;
pub const GAP: f32 = 12.;

/// At 175% DPI the content area is still measured in logical pixels, so the
/// 280 px ceiling matches Explorer's reference screenshot.
pub fn columns(available: f32, has_image: bool) -> ContentColumns {
    let available = available.max(0.);
    let middle = if has_image && available >= 760. {
        Some(MIDDLE_WIDTH)
    } else {
        None
    };
    let reserved = middle.map_or(0., |width| width + GAP);
    // Keep a usable second column only while the first column can still show
    // text next to its icon. Below that point Explorer hides date/size.
    let show_right = available >= 400. + reserved;
    if !show_right {
        return ContentColumns {
            first: available,
            middle: None,
            right: None,
        };
    }
    let first = (available - reserved - RIGHT_MIN - GAP).clamp(FIRST_MIN, FIRST_MAX);
    ContentColumns {
        first,
        middle,
        right: Some((available - reserved - first - GAP).max(0.)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_column_grows_to_the_ceiling_then_stops() {
        let edge = columns(400., false);
        assert!(edge.first >= FIRST_MIN && edge.first < FIRST_MAX);
        assert_eq!(columns(500., false).first, FIRST_MAX);
        assert_eq!(columns(1400., false).first, FIRST_MAX);
        assert!(columns(430., false).first > columns(400., false).first);
    }

    #[test]
    fn narrow_windows_drop_the_secondary_columns() {
        let narrow = columns(380., false);
        assert_eq!(narrow.first, 380.);
        assert_eq!(narrow.right, None);
        assert_eq!(narrow.middle, None);
    }

    #[test]
    fn image_middle_column_only_appears_when_there_is_room() {
        assert_eq!(columns(700., true).middle, None);
        let wide = columns(1000., true);
        assert_eq!(wide.first, FIRST_MAX);
        assert_eq!(wide.middle, Some(MIDDLE_WIDTH));
        assert!(wide.right.unwrap() > RIGHT_MIN);
    }
}

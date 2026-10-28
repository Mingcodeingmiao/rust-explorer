//! Logical sizes before the OS DPI scale and optional application zoom.
pub const COMPACT_FILE_ROW: f32 = 26.;
pub const DRIVE_TILE_WIDTH: f32 = 250.;
pub const DRIVE_TILE_HEIGHT: f32 = 55.;
pub const DRIVE_TILE_ICON: f32 = 40. * 1.17;
pub const DRIVE_TILE_BAR_HEIGHT: f32 = 12. * 1.2;
pub const DRIVE_TILE_BAR_WIDTH: f32 = 186. * 1.03;
pub const DRIVE_TILE_GAP: f32 = DRIVE_TILE_WIDTH - 8. - DRIVE_TILE_ICON - DRIVE_TILE_BAR_WIDTH;
pub const ADDRESS_ROW_HEIGHT: f32 = 43.;
pub const TAB_MAX_WIDTH: f32 = 230.;
pub const CONTENT_FIRST_COLUMN_MAX: f32 = 160.;

pub fn content_columns(inner_width: f32, image: bool) -> (f32, bool) {
    let layout = crate::content_layout::columns(inner_width, image);
    (layout.first, layout.right.is_some())
}

pub fn recycled_row_count(viewport_height: f32, zoom: f32, total_rows: usize) -> usize {
    let zoom = zoom.max(0.25);
    total_rows.min(((viewport_height.max(0.) * zoom + 320.) / (26. * zoom)).ceil() as usize + 2)
}

/// Whether the drawn link badge is needed for an item whose current artwork is a
/// thumbnail (`has_thumbnail`) or the Shell bitmap fallback.
///
/// A thumbnail of a shortcut or reparse link shows the target's picture and has no
/// overlay, so the marker is drawn on top. The Shell bitmap already owns the arrow
/// for `.lnk`/`.url`; only reparse-point links need the marker there. `.url` icons
/// always draw their own arrow, so they never get a drawn badge.
pub fn needs_link_badge(link: bool, has_thumbnail: bool, shortcut: bool, url: bool) -> bool {
    if !link || url {
        return false;
    }
    if has_thumbnail {
        return true;
    }
    !shortcut
}

/// Whether a right click targets one item cell. An unselected item only answers
/// a hit on its icon, text or metadata regions; an item that is already
/// selected answers anywhere inside its own cell.
pub fn right_click_hits(
    regions: &[(f32, f32, f32, f32)],
    x: f32,
    y: f32,
    cell_width: f32,
    cell_height: f32,
    selected: bool,
) -> bool {
    let in_region = regions.iter().any(|(left, top, width, height)| {
        x >= *left && x <= left + width && y >= *top && y <= top + height
    });
    in_region || (selected && x >= 0. && y >= 0. && x <= cell_width && y <= cell_height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn link_badges_follow_the_artwork_that_is_actually_shown() {
        // Thumbnails show the target's picture: any link needs the marker.
        assert!(needs_link_badge(true, true, true, false));
        assert!(needs_link_badge(true, true, false, false));
        // The Shell bitmap of a .lnk already carries the arrow; a junction does not.
        assert!(!needs_link_badge(true, false, true, false));
        assert!(needs_link_badge(true, false, false, false));
        // .url never gets a drawn badge, and plain items never do either.
        assert!(!needs_link_badge(true, true, true, true));
        assert!(!needs_link_badge(true, false, true, true));
        assert!(!needs_link_badge(false, true, false, false));
    }
    #[test]
    fn requested_chrome_and_drive_ratios_fit_fixed_tiles() {
        assert!((DRIVE_TILE_ICON / 40. - 1.17).abs() < 0.001);
        assert!((DRIVE_TILE_BAR_WIDTH / 186. - 1.03).abs() < 0.001);
        assert!((DRIVE_TILE_BAR_HEIGHT / 12. - 1.2).abs() < 0.001);
        assert!(
            (8. + DRIVE_TILE_ICON + DRIVE_TILE_GAP + DRIVE_TILE_BAR_WIDTH - DRIVE_TILE_WIDTH).abs()
                < 0.001
        );
        assert_eq!(TAB_MAX_WIDTH, 200. * 1.15);
        assert_eq!((ADDRESS_ROW_HEIGHT - 32.) * 2., 54. - 32.);
    }
    #[test]
    fn drive_tile_matches_the_175_percent_reference() {
        assert_eq!((DRIVE_TILE_WIDTH * 1.75).round(), 438.);
        assert_eq!((DRIVE_TILE_HEIGHT * 1.75).round(), 96.);
    }
    #[test]
    fn recycled_pool_stays_bounded_for_large_directories() {
        assert_eq!(recycled_row_count(600., 1., 0), 0);
        assert_eq!(recycled_row_count(600., 1., 8), 8);
        assert_eq!(recycled_row_count(600., 1., 100_000), 38);
        for zoom in [0.5, 1., 1.75, 2.] {
            let pool = recycled_row_count(600., zoom, 100_000);
            assert!((pool - 2) as f32 * 26. * zoom >= 600. * zoom + 320.);
        }
    }
    #[test]
    fn right_click_answers_the_whole_cell_only_for_a_selected_item() {
        // One text region: icon plus name on a details row.
        let regions = [(8., 4., 120., 26.)];
        assert!(right_click_hits(&regions, 20., 10., 600., 34., false));
        // Empty space of an unselected row stays with the background menu.
        assert!(!right_click_hits(&regions, 400., 10., 600., 34., false));
        // The same click on a selected row targets the item.
        assert!(right_click_hits(&regions, 400., 10., 600., 34., true));
        // Outside the cell is never the item, even when selected.
        assert!(!right_click_hits(&regions, 601., 10., 600., 34., true));
        assert!(!right_click_hits(&regions, 400., 35., 600., 34., true));
        assert!(!right_click_hits(&regions, -1., 10., 600., 34., true));
    }

    #[test]
    fn content_columns_cap_at_reference_dpi_and_hide_metadata_when_narrow() {
        let (wide, metadata) = content_columns(900., true);
        assert_eq!(wide * 1.75, 280.);
        assert!(metadata);
        let (narrow, metadata) = content_columns(280., false);
        assert!(narrow < wide);
        assert!(!metadata);
        assert!(!content_columns(400., true).1);
    }
}

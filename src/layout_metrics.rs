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
    let width = inner_width.max(0.);
    (
        (width * 0.45).min(CONTENT_FIRST_COLUMN_MAX),
        width >= if image { 480. } else { 360. },
    )
}

pub fn recycled_row_count(viewport_height: f32, zoom: f32, total_rows: usize) -> usize {
    let zoom = zoom.max(0.25);
    total_rows.min(((viewport_height.max(0.) * zoom + 320.) / (26. * zoom)).ceil() as usize + 2)
}

#[cfg(test)]
mod tests {
    use super::*;
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

//! Logical sizes before the OS DPI scale and optional application zoom.
pub const COMPACT_FILE_ROW: f32 = 26.;
pub const DRIVE_TILE_WIDTH: f32 = 250.;
pub const DRIVE_TILE_HEIGHT: f32 = 55.;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_tile_matches_the_175_percent_reference() {
        assert_eq!((DRIVE_TILE_WIDTH * 1.75).round(), 438.);
        assert_eq!((DRIVE_TILE_HEIGHT * 1.75).round(), 96.);
    }
}

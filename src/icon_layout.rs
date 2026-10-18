//! Pure geometry for small provider images inside Explorer's fixed icon slots.
/// Wrap at supplied grapheme boundaries, without inserting invisible characters
/// or asking the Windows text backend to slice a clamped UTF-8 run.
pub fn label_lines<'a>(
    parts: impl IntoIterator<Item = &'a str>,
    width: f32,
    limit: usize,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    if limit == 0 {
        return vec![];
    }
    let mut lines = vec![String::new()];
    for part in parts {
        let last = lines.len() - 1;
        let candidate = format!("{}{part}", lines[last]);
        if lines[last].is_empty() || measure(&candidate) <= width {
            lines[last] = candidate;
        } else if lines.len() < limit {
            lines.push(part.to_owned());
        } else {
            while !lines[last].is_empty() && measure(&format!("{}…", lines[last])) > width {
                lines[last].pop();
            }
            lines[last].push('…');
            break;
        }
    }
    lines
}

#[cfg(test)]
mod label_tests {
    use super::*;
    #[test]
    fn multibyte_names_are_bounded_without_zero_width_insertions() {
        let parts = ["文", "件", "夹", "😀", "a", "b", "c", "d", "e"];
        let lines = label_lines(parts, 2., 4, |s| s.chars().count() as f32);
        assert_eq!(lines, ["文件", "夹😀", "ab", "c…"]);
        assert!(lines.iter().all(|line| !line.contains('\u{200b}')));
    }
}

pub fn thumbnail_edge(logical: f32, width: u32, height: u32, scale: f32, folder: bool) -> f32 {
    // Shell can return a 48px cached folder composition for a jumbo request.
    // Its bitmap resolution must not change the folder's visual slot size.
    // Photos and other document thumbnails still retain their native-size rule.
    if folder || width == 0 || height == 0 {
        return logical;
    }
    (width.max(height) as f32 / scale.max(0.5)).clamp(logical / 3., logical)
}

/// Jumbo image lists sometimes contain a 32px legacy icon in a 256px canvas.
/// Center that real artwork and frame it rather than pinning it to the top.
pub fn center_legacy_icon(size: u32, rgba: Vec<u8>) -> Vec<u8> {
    if size < 64 || rgba.len() != (size * size * 4) as usize {
        return rgba;
    }
    let (mut left, mut top, mut right, mut bottom) = (size, size, 0, 0);
    for y in 0..size {
        for x in 0..size {
            if rgba[((y * size + x) * 4 + 3) as usize] > 8 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x + 1);
                bottom = bottom.max(y + 1);
            }
        }
    }
    if left >= right || top >= bottom || (right - left).max(bottom - top) * 3 > size * 2 {
        return rgba;
    }
    let original = image::RgbaImage::from_raw(size, size, rgba).unwrap();
    let crop =
        image::imageops::crop_imm(&original, left, top, right - left, bottom - top).to_image();
    let edge = ((right - left).max(bottom - top) + 4)
        .max(size / 3)
        .min(size);
    let factor = (edge.saturating_sub(4).max(1)) as f32 / crop.width().max(crop.height()) as f32;
    let width = (crop.width() as f32 * factor).round().max(1.) as u32;
    let height = (crop.height() as f32 * factor).round().max(1.) as u32;
    let picture =
        image::imageops::resize(&crop, width, height, image::imageops::FilterType::Lanczos3);
    let mut result = image::RgbaImage::new(size, size);
    image::imageops::overlay(
        &mut result,
        &picture,
        ((size - width) / 2) as i64,
        ((size - height) / 2) as i64,
    );
    let (x, y) = ((size - edge) / 2, (size - edge) / 2);
    let frame = image::Rgba([116, 116, 116, 210]);
    for i in 0..edge {
        result.put_pixel(x + i, y, frame);
        result.put_pixel(x + i, y + edge - 1, frame);
        result.put_pixel(x, y + i, frame);
        result.put_pixel(x + edge - 1, y + i, frame);
    }
    result.into_raw()
}

/// Older folder-thumbnail providers return an opaque black canvas rather than
/// alpha. Remove only near-black pixels connected to that canvas's boundary.
/// Existing transparency and enclosed black photo/details are kept intact.
pub fn transparent_folder_border(width: u32, height: u32, mut rgba: Vec<u8>) -> Vec<u8> {
    let Some(count) = (width as usize).checked_mul(height as usize) else {
        return rgba;
    };
    if width < 2
        || height < 2
        || count.checked_mul(4) != Some(rgba.len())
        || rgba.chunks_exact(4).any(|pixel| pixel[3] != 255)
    {
        return rgba;
    }
    let black = |index: usize| {
        rgba[index * 4..index * 4 + 3]
            .iter()
            .all(|value| *value <= 2)
    };
    let corners = [0, width as usize - 1, count - width as usize, count - 1];
    if corners.iter().filter(|index| black(**index)).count() < 3 || (0..count).all(black) {
        return rgba;
    }
    let mut visited = vec![false; count];
    let mut queue = std::collections::VecDeque::new();
    let width = width as usize;
    let height = height as usize;
    for y in 0..height {
        for x in 0..width {
            if (x == 0 || x + 1 == width || y == 0 || y + 1 == height) && black(y * width + x) {
                visited[y * width + x] = true;
                queue.push_back(y * width + x);
            }
        }
    }
    while let Some(index) = queue.pop_front() {
        let (x, y) = (index % width, index / width);
        let neighbors = [
            (x > 0).then(|| index - 1),
            (x + 1 < width).then_some(index + 1),
            (y > 0).then(|| index - width),
            (y + 1 < height).then_some(index + width),
        ];
        for next in neighbors.into_iter().flatten() {
            if !visited[next] && black(next) {
                visited[next] = true;
                queue.push_back(next);
            }
        }
    }
    for (index, clear) in visited.into_iter().enumerate() {
        if clear {
            rgba[index * 4..index * 4 + 4].fill(0);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_scale_respects_native_size_and_minimum_area() {
        assert_eq!(thumbnail_edge(150., 8, 12, 1., false), 50.);
        assert_eq!(thumbnail_edge(150., 96, 60, 1., false), 96.);
        assert_eq!(thumbnail_edge(150., 400, 200, 1., false), 150.);
        assert_eq!(thumbnail_edge(150., 192, 120, 2., false), 96.);
    }
    #[test]
    fn cached_folder_resolution_does_not_shrink_jumbo_folder() {
        for scale in [1., 1.5, 2.] {
            assert_eq!(thumbnail_edge(240., 48, 48, scale, true), 240.);
            assert_eq!(thumbnail_edge(240., 256, 256, scale, true), 240.);
        }
        assert_eq!(thumbnail_edge(240., 48, 48, 1., false), 80.);
    }
    #[test]
    fn legacy_artwork_is_centered_and_full_size_art_is_untouched() {
        let mut rgba = vec![0; 256 * 256 * 4];
        for y in 0..32 {
            for x in 0..32 {
                rgba[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].copy_from_slice(&[255, 160, 0, 255]);
            }
        }
        let centered = center_legacy_icon(256, rgba);
        assert_eq!(centered[3], 0);
        assert!(centered[(128 * 256 + 128) * 4 + 3] > 0);
        let full = vec![255; 128 * 128 * 4];
        assert_eq!(center_legacy_icon(128, full.clone()), full);
    }
    #[test]
    fn folder_canvas_is_removed_without_erasing_enclosed_black_content() {
        let mut image = [0, 0, 0, 255].repeat(49);
        for y in 1..6 {
            for x in 1..6 {
                image[(y * 7 + x) * 4..(y * 7 + x) * 4 + 4].copy_from_slice(&[230, 170, 30, 255]);
            }
        }
        image[(3 * 7 + 3) * 4..(3 * 7 + 3) * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
        let result = transparent_folder_border(7, 7, image);
        assert_eq!(&result[..4], &[0, 0, 0, 0]);
        assert_eq!(
            &result[(3 * 7 + 3) * 4..(3 * 7 + 3) * 4 + 4],
            &[0, 0, 0, 255]
        );
        assert_eq!(
            &result[(2 * 7 + 2) * 4..(2 * 7 + 2) * 4 + 4],
            &[230, 170, 30, 255]
        );
    }
    #[test]
    fn folder_alpha_preserves_transparency_and_opaque_black_artwork() {
        let black = [0, 0, 0, 255].repeat(16);
        assert_eq!(transparent_folder_border(4, 4, black.clone()), black);
        let mut alpha = [0, 0, 0, 255].repeat(16);
        alpha[3] = 80;
        alpha[20] = 210;
        assert_eq!(transparent_folder_border(4, 4, alpha.clone()), alpha);
    }
}

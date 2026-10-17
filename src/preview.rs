use anyhow::{Result, bail};
use std::{fs::File, io::Read, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Video,
    Pdf,
    Font,
    Unsupported,
}
/// The render thread can inspect the extension; target discovery belongs to
/// the preview worker and its cancellable native helper.
pub fn is_shortcut(path: &Path) -> bool {
    path.extension().is_some_and(|value| {
        let value = value.to_string_lossy();
        value.eq_ignore_ascii_case("lnk") || value.eq_ignore_ascii_case("url")
    })
}
/// Resolve before deciding the allowlisted preview type. Ordinary files avoid
/// a helper round-trip; shortcuts never execute the destination application.
pub fn resolve_source(
    path: &Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<std::path::PathBuf> {
    if is_shortcut(path) {
        crate::native_assets::resolve_shortcut(path, cancel)
    } else {
        Ok(path.to_path_buf())
    }
}
pub fn kind(path: &Path) -> Kind {
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "md" | "rs" | "toml" | "json" | "yaml" | "yml" | "xml" | "csv" | "log" | "ini"
        | "cfg" | "c" | "h" | "cpp" | "py" | "js" | "ts" | "css" | "html" | "sql" | "ps1"
        | "sh" | "bat" | "gitignore" => Kind::Text,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "ico" => Kind::Image,
        "mp4" | "m4v" | "webm" | "mov" | "mkv" | "avi" | "wmv" => Kind::Video,
        "pdf" => Kind::Pdf,
        "ttf" | "otf" | "ttc" | "fon" => Kind::Font,
        _ => Kind::Unsupported,
    }
}
pub struct TextPreview {
    pub lines: Vec<String>,
    pub truncated: bool,
}
pub fn text(path: &Path) -> Result<TextPreview> {
    const LIMIT: usize = 256 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > LIMIT;
    bytes.truncate(LIMIT);
    let text = if bytes.starts_with(&[0xff, 0xfe]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|v| u16::from_be_bytes([v[0], v[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        if bytes.contains(&0) {
            bail!("二进制内容，无法作为文本预览");
        }
        String::from_utf8_lossy(&bytes)
            .trim_start_matches('\u{feff}')
            .to_owned()
    };
    let lines = text
        .lines()
        .take(6000)
        .map(|v| v.chars().take(2000).collect())
        .collect();
    Ok(TextPreview {
        lines,
        truncated: truncated || text.lines().count() > 6000,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_allowlist() {
        assert_eq!(kind(Path::new("x.PDF")), Kind::Pdf);
        assert_eq!(kind(Path::new("x.exe")), Kind::Unsupported);
        assert_eq!(kind(Path::new("x.svg")), Kind::Unsupported);
        assert_eq!(kind(Path::new("x.OTF")), Kind::Font);
        assert_eq!(kind(Path::new("x.ttc")), Kind::Font);
    }
}

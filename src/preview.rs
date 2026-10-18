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
    let text = decode(&bytes)?;
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

fn utf16(bytes: &[u8], little_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|value| {
            if little_endian {
                u16::from_le_bytes([value[0], value[1]])
            } else {
                u16::from_be_bytes([value[0], value[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// Pick an encoding without relying on the system ANSI code page. BOMs win,
/// then UTF-8, then a NUL-pattern check for BOM-less UTF-16, and finally
/// GB18030, which is a superset of GB2312/GBK used by Chinese text files.
fn decode(bytes: &[u8]) -> Result<String> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Ok(String::from_utf8_lossy(&bytes[3..])
            .trim_start_matches('\u{feff}')
            .to_owned());
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        return Ok(utf16(&bytes[2..], true)
            .trim_start_matches('\u{feff}')
            .to_owned());
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        return Ok(utf16(&bytes[2..], false)
            .trim_start_matches('\u{feff}')
            .to_owned());
    }
    if bytes.contains(&0) {
        let even_nuls = bytes.iter().step_by(2).filter(|byte| **byte == 0).count();
        let odd_nuls = bytes
            .iter()
            .skip(1)
            .step_by(2)
            .filter(|byte| **byte == 0)
            .count();
        let pairs = bytes.len() / 2;
        if pairs > 2 && odd_nuls * 4 > pairs * 3 && even_nuls * 8 < pairs {
            return Ok(utf16(bytes, true).trim_start_matches('\u{feff}').to_owned());
        }
        if pairs > 2 && even_nuls * 4 > pairs * 3 && odd_nuls * 8 < pairs {
            return Ok(utf16(bytes, false)
                .trim_start_matches('\u{feff}')
                .to_owned());
        }
        bail!("二进制内容，无法作为文本预览");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.trim_start_matches('\u{feff}').to_owned());
    }
    let (text, _, _) = encoding_rs::GB18030.decode(bytes);
    Ok(text.trim_start_matches('\u{feff}').to_owned())
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

    #[test]
    fn decodes_utf8_utf16_and_gb2312() {
        assert_eq!(decode("纯文本".as_bytes()).unwrap(), "纯文本");
        assert_eq!(
            decode(&[0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65]).unwrap(),
            "中文"
        );
        // "中文测试" in GB2312/GBK.
        assert_eq!(
            decode(&[0xd6, 0xd0, 0xce, 0xc4, 0xb2, 0xe2, 0xca, 0xd4]).unwrap(),
            "中文测试"
        );
        assert!(decode(&[0x00, 0x01, 0x02, 0x00]).is_err());
    }
}

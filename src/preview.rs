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
/// `.url` Shell icons already carry the link arrow, so a drawn badge would be a
/// second arrow. `.lnk` keeps the drawn badge: its large-view thumbnail is the
/// target icon, which has no overlay of its own.
pub fn is_url_shortcut(path: &Path) -> bool {
    path.extension()
        .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("url"))
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
    pub encoding: &'static str,
}
pub fn text(path: &Path) -> Result<TextPreview> {
    const LIMIT: usize = 256 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > LIMIT;
    bytes.truncate(LIMIT);
    let (text, encoding) = decode_text(&bytes, truncated)?;
    let lines = text
        .lines()
        .take(6000)
        .map(|v| v.chars().take(2000).collect())
        .collect();
    Ok(TextPreview {
        lines,
        truncated: truncated || text.lines().count() > 6000,
        encoding,
    })
}

fn decode_text(bytes: &[u8], truncated: bool) -> Result<(String, &'static str)> {
    let little = bytes.starts_with(&[0xff, 0xfe]);
    if little || bytes.starts_with(&[0xfe, 0xff]) {
        if !truncated && (bytes.len() - 2) % 2 != 0 {
            bail!("UTF-16 文本末尾不完整");
        }
        let mut units: Vec<_> = bytes[2..]
            .chunks_exact(2)
            .map(|v| {
                if little {
                    u16::from_le_bytes([v[0], v[1]])
                } else {
                    u16::from_be_bytes([v[0], v[1]])
                }
            })
            .collect();
        if truncated && units.last().is_some_and(|v| (0xd800..=0xdbff).contains(v)) {
            units.pop();
        }
        return Ok((
            String::from_utf16(&units)?,
            if little { "UTF-16 LE" } else { "UTF-16 BE" },
        ));
    }
    if bytes.contains(&0) && bytes.len() >= 8 {
        let pairs = bytes.len() / 2;
        let even = bytes.iter().step_by(2).filter(|b| **b == 0).count();
        let odd = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        let little = odd * 4 > pairs * 3 && even * 8 < pairs;
        let big = even * 4 > pairs * 3 && odd * 8 < pairs;
        if little || big {
            let mut marked = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            marked.extend_from_slice(bytes);
            return decode_text(&marked, truncated);
        }
    }
    if bytes.contains(&0)
        || bytes
            .iter()
            .any(|b| *b < 0x20 && !matches!(*b, b'\t' | b'\r' | b'\n' | 0x0c))
    {
        bail!("二进制内容，无法作为文本预览");
    }
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    let bytes = if bom { &bytes[3..] } else { bytes };
    match std::str::from_utf8(bytes) {
        Ok(text) => return Ok((text.to_owned(), "UTF-8")),
        Err(error) if truncated && error.error_len().is_none() => {
            return Ok((
                std::str::from_utf8(&bytes[..error.valid_up_to()])?.to_owned(),
                "UTF-8",
            ));
        }
        Err(_) if bom => bail!("UTF-8 BOM 文本含无效字符，无法可靠解码"),
        Err(_) => {}
    }
    // Windows CP936 includes GB2312. Never use the machine's locale-dependent ACP.
    use windows::Win32::Globalization::{MB_ERR_INVALID_CHARS, MultiByteToWideChar};
    for trim in 0..=usize::from(truncated) {
        let Some(end) = bytes.len().checked_sub(trim) else {
            continue;
        };
        let input = &bytes[..end];
        if input.is_empty() {
            continue;
        }
        let mut output = vec![0u16; input.len()];
        let count =
            unsafe { MultiByteToWideChar(936, MB_ERR_INVALID_CHARS, input, Some(&mut output)) };
        if count > 0 {
            output.truncate(count as usize);
            return Ok((String::from_utf16(&output)?, "GBK / GB2312（推测）"));
        }
    }
    // Keep the contributor's GB18030 support, but reject replacement characters
    // instead of silently accepting damaged input. A bounded read may cut a
    // four-byte sequence, so retry only the final three bytes.
    for trim in 0..=if truncated { 3 } else { 0 } {
        if let Some(end) = bytes.len().checked_sub(trim) {
            if let Some(text) = encoding_rs::GB18030
                .decode_without_bom_handling_and_without_replacement(&bytes[..end])
            {
                return Ok((text.into_owned(), "GB18030（推测）"));
            }
        }
    }
    bail!("无法可靠识别文本编码；支持 UTF-8、UTF-16、GBK/GB2312 和 GB18030")
}
#[cfg(test)]
fn decode_for_test(bytes: &[u8]) -> Result<String> {
    decode_text(bytes, false).map(|v| v.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integrated_decoder_keeps_gb18030_and_bomless_utf16_strict() {
        let text = "ASCII text with a UTF-16 signature pattern";
        let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_text(&bytes, false).unwrap().0, text);
        let (bytes, _, errors) = encoding_rs::GB18030.encode("𠀀");
        assert!(!errors);
        assert_eq!(decode_text(&bytes, false).unwrap().0, "𠀀");
        assert!(decode_text(&[0xef, 0xbb, 0xbf, 0xff], false).is_err());
    }
    #[test]
    fn chinese_encodings_and_unicode_boms() {
        assert_eq!(
            decode_text(b"\xd6\xd0\xce\xc4\r\nabc", false).unwrap().0,
            "中文\r\nabc"
        );
        assert_eq!(
            decode_text("中文😀".as_bytes(), false).unwrap(),
            ("中文😀".into(), "UTF-8")
        );
        assert_eq!(decode_text(b"\xef\xbb\xbfhello", false).unwrap().0, "hello");
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in "中文😀\nabc".encode_utf16() {
                bytes.extend(if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            assert_eq!(decode_text(&bytes, false).unwrap().0, "中文😀\nabc");
        }
    }
    #[test]
    fn cut_multibyte_tail_does_not_change_utf8_to_gbk() {
        assert_eq!(
            decode_text(b"abc\xe4\xb8", true).unwrap(),
            ("abc".into(), "UTF-8")
        );
        assert_eq!(decode_text(b"\xd6\xd0\xce", true).unwrap().0, "中");
        assert_eq!(
            decode_text(b"\xff\xfe\x2d\x4e\x3d\xd8", true).unwrap().0,
            "中"
        );
        assert!(decode_text(b"\xff\xfe\x2d", false).is_err());
        assert!(decode_text(b"\xef\xbb\xbf\xff", false).is_err());
    }
    #[test]
    fn empty_and_binary_inputs() {
        assert_eq!(decode_text(b"", false).unwrap().0, "");
        assert!(decode_text(b"MZ\0binary", false).is_err());
        assert!(decode_text(b"\x01\x02\x03", false).is_err());
        assert!(decode_text(b"\xff\xff", false).is_err());
    }
    #[test]
    fn file_preview_preserves_source_and_limits_multibyte_reads() {
        let root = std::env::temp_dir().join(format!(
            "rust-explorer-encoding-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("sample.txt");
        let original = b"\xd6\xd0\xce\xc4\r\nhello";
        std::fs::write(&path, original).unwrap();
        let result = text(&path).unwrap();
        assert_eq!(result.lines, ["中文", "hello"]);
        assert!(!result.truncated);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut large = vec![b'a'; 256 * 1024 - 1];
        large.extend("中tail".as_bytes());
        std::fs::write(&path, &large).unwrap();
        let result = text(&path).unwrap();
        assert!(result.truncated);
        assert_eq!(result.encoding, "UTF-8");
        assert_eq!(std::fs::read(&path).unwrap(), large);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
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
        assert_eq!(decode_for_test("纯文本".as_bytes()).unwrap(), "纯文本");
        assert_eq!(
            decode_for_test(&[0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65]).unwrap(),
            "中文"
        );
        // "中文测试" in GB2312/GBK.
        assert_eq!(
            decode_for_test(&[0xd6, 0xd0, 0xce, 0xc4, 0xb2, 0xe2, 0xca, 0xd4]).unwrap(),
            "中文测试"
        );
        assert!(decode_for_test(&[0x00, 0x01, 0x02, 0x00]).is_err());
    }
}

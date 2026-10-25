//! Embedded four-language catalogs. No disk/network/COM work during rendering.
use std::{
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Chinese,
    Japanese,
    English,
    Korean,
}
impl Language {
    pub fn from_tag(tag: &str) -> Self {
        match tag
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "zh" => Self::Chinese,
            "ja" => Self::Japanese,
            "ko" => Self::Korean,
            _ => Self::English,
        }
    }
    pub fn from_langid(id: u16) -> Self {
        match id & 0x03ff {
            // All Chinese locales (Simplified, Traditional, Hong Kong, etc.)
            // use the single zh-CN catalog.
            0x04 => Self::Chinese,
            0x11 => Self::Japanese,
            0x12 => Self::Korean,
            _ => Self::English,
        }
    }
    /// UI language selected by Windows on first launch.
    pub fn system() -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::Globalization::GetUserDefaultUILanguage;
            Self::from_langid(unsafe { GetUserDefaultUILanguage() } as u16)
        }
        #[cfg(not(windows))]
        {
            Self::English
        }
    }
    pub fn native_name(self) -> &'static str {
        match self {
            Self::Chinese => "简体中文",
            Self::Japanese => "日本語",
            Self::English => "English",
            Self::Korean => "한국어",
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            Self::Chinese => "zh-CN",
            Self::Japanese => "ja-JP",
            Self::English => "en-US",
            Self::Korean => "ko-KR",
        }
    }
}
static LANGUAGE: AtomicU8 = AtomicU8::new(0);
pub fn set_language(tag: &str) {
    let language = Language::from_tag(tag);
    LANGUAGE.store(
        match language {
            Language::English => 0,
            Language::Chinese => 1,
            Language::Japanese => 2,
            Language::Korean => 3,
        },
        Ordering::Relaxed,
    );
}
pub fn language() -> Language {
    // Diagnostic override is read once, without accessing Windows settings.
    static OVERRIDE: OnceLock<Option<Language>> = OnceLock::new();
    if let Some(language) = *OVERRIDE.get_or_init(|| {
        std::env::var("RUST_EXPLORER_LANGUAGE")
            .ok()
            .map(|tag| Language::from_tag(&tag))
    }) {
        return language;
    }
    match LANGUAGE.load(Ordering::Relaxed) {
        1 => Language::Chinese,
        2 => Language::Japanese,
        3 => Language::Korean,
        _ => Language::English,
    }
}
type Catalog = HashMap<String, String>;
fn catalog(language: Language) -> &'static Catalog {
    static ZH: OnceLock<Catalog> = OnceLock::new();
    static EN: OnceLock<Catalog> = OnceLock::new();
    static JA: OnceLock<Catalog> = OnceLock::new();
    static KO: OnceLock<Catalog> = OnceLock::new();
    let (slot, data) = match language {
        Language::Chinese => (&ZH, include_str!("../resources/locales/zh-CN.json")),
        Language::Japanese => (&JA, include_str!("../resources/locales/ja-JP.json")),
        Language::English => (&EN, include_str!("../resources/locales/en-US.json")),
        Language::Korean => (&KO, include_str!("../resources/locales/ko-KR.json")),
    };
    slot.get_or_init(|| serde_json::from_str(data).expect("validated embedded language catalog"))
}
pub fn text(source: &str) -> &str {
    text_in(language(), source)
}
/// Translate generated metadata only; never pass a user filename here.
pub fn kind(value: &str) -> String {
    if let Some(extension) = value.strip_suffix(" 文件") {
        render("{} 文件", &[("0", extension.to_owned())])
    } else {
        text(value).to_owned()
    }
}
pub fn text_in(language: Language, source: &str) -> &str {
    catalog(language)
        .get(source)
        .or_else(|| catalog(Language::English).get(source))
        .map(String::as_str)
        .unwrap_or(source)
}
/// Used only for presentation controls that historically choose icons by labels.
/// Never use this to rewrite paths or user filenames.
pub fn source(label: &str) -> &str {
    static REVERSE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    REVERSE
        .get_or_init(|| {
            // Only icon-bearing button labels need reverse lookup. General catalog
            // values can legitimately coincide (e.g. View and Show).
            [
                "⊕ 新建",
                "⌂ 主文件夹",
                "▣ 预览",
                "□ 预览",
                "新建",
                "剪切",
                "复制",
                "粘贴",
                "重命名",
                "共享",
                "删除",
                "预览",
                "主文件夹",
                "桌面",
                "下载",
                "文档",
                "图片",
                "音乐",
                "视频",
            ]
            .into_iter()
            .map(|key| (text(key), key))
            .collect()
        })
        .get(label)
        .copied()
        .unwrap_or(label)
}
pub fn wide(source: &'static str) -> windows::core::PCWSTR {
    type WideCache = std::sync::Mutex<HashMap<String, Box<[u16]>>>;
    static WIDE: OnceLock<WideCache> = OnceLock::new();
    let mut cache = WIDE.get_or_init(Default::default).lock().unwrap();
    let value = cache
        .entry(source.into())
        .or_insert_with(|| text(source).encode_utf16().chain(Some(0)).collect());
    // Entries are never removed; the boxed allocation outlives every native menu.
    windows::core::PCWSTR(value.as_ptr())
}

/// Render already formatted values verbatim. Reordering belongs to the catalog;
/// filenames containing braces or translated words remain untouched.
pub fn render(source: &str, values: &[(&str, String)]) -> String {
    render_in(language(), source, values)
}
pub fn render_in(language: Language, source: &str, values: &[(&str, String)]) -> String {
    let template = text_in(language, source);
    let mut result = String::new();
    let mut index = 0;
    let mut implicit = 0;
    while index < template.len() {
        let tail = &template[index..];
        if tail.starts_with("{{") {
            result.push('{');
            index += 2;
        } else if tail.starts_with("}}") {
            result.push('}');
            index += 2;
        } else if tail.starts_with('{') {
            let Some(end) = tail.find('}') else {
                result.push_str(tail);
                break;
            };
            let field = &tail[1..end];
            let key = if field.starts_with(':') || field.is_empty() {
                let key = format!("{implicit}{field}");
                implicit += 1;
                key
            } else {
                field.to_string()
            };
            if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
                result.push_str(value);
            } else {
                result.push_str(&tail[..=end]);
            }
            index += end + 1;
        } else {
            let ch = tail.chars().next().unwrap();
            result.push(ch);
            index += ch.len_utf8();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(template: &str) -> Vec<String> {
        let mut fields = Vec::new();
        let mut cursor = 0;
        let mut implicit = 0;
        while cursor < template.len() {
            let tail = &template[cursor..];
            if tail.starts_with("{{") || tail.starts_with("}}") {
                cursor += 2;
            } else if tail.starts_with('{') {
                let end = tail.find('}').expect("closed placeholder");
                let field = &tail[1..end];
                fields.push(if field.is_empty() || field.starts_with(':') {
                    let field = format!("{implicit}{field}");
                    implicit += 1;
                    field
                } else {
                    field.into()
                });
                cursor += end + 1;
            } else {
                cursor += tail.chars().next().unwrap().len_utf8();
            }
        }
        fields.sort();
        fields
    }
    #[test]
    fn catalogs_have_matching_keys_and_format_arguments() {
        let source = catalog(Language::Chinese);
        for language in [
            Language::Chinese,
            Language::English,
            Language::Japanese,
            Language::Korean,
        ] {
            let translated = catalog(language);
            assert_eq!(source.len(), translated.len());
            for (key, original) in source {
                let value = translated.get(key).expect(key);
                assert!(!value.trim().is_empty(), "{}: {key}", language.tag());
                assert_eq!(fields(original), fields(value), "{}: {key}", language.tag());
            }
        }
    }
    #[test]
    fn display_language_and_fallback() {
        for (tag, expected) in [
            ("zh-TW", Language::Chinese),
            ("ja_JP", Language::Japanese),
            ("ko-KR", Language::Korean),
            ("en-GB", Language::English),
            ("fr-FR", Language::English),
        ] {
            assert_eq!(Language::from_tag(tag), expected);
        }
        for (id, expected) in [
            (0x0804, Language::Chinese),
            (0x0404, Language::Chinese),
            (0x0c04, Language::Chinese),
            (0x1004, Language::Chinese),
            (0x0411, Language::Japanese),
            (0x0412, Language::Korean),
            (0x0409, Language::English),
            (0x040c, Language::English),
        ] {
            assert_eq!(Language::from_langid(id), expected);
        }
        assert_eq!(
            text_in(Language::English, "unregistered filename.txt"),
            "unregistered filename.txt"
        );
    }
    #[test]
    fn translations_reorder_values_without_rewriting_paths() {
        let filename = r"C:\文件夹\{0}日本語한국어.txt";
        assert_eq!(
            render_in(Language::English, "类型：{}", &[("0", filename.into())]),
            format!("Type: {filename}")
        );
        assert_eq!(
            render_in(
                Language::Korean,
                "{} 可用，共 {}",
                &[("0", "2 GB".into()), ("1", "10 GB".into())]
            ),
            "10 GB 중 2 GB 사용 가능"
        );
        assert_eq!(
            render_in(
                Language::English,
                "{{literal}} {0}",
                &[("0", filename.into())]
            ),
            format!("{{literal}} {filename}")
        );
    }
    #[test]
    fn format_macro_preserves_borrows_named_values_and_evaluates_once() {
        let value = String::from("unchanged {name}.txt");
        let mut calls = 0;
        let actual = crate::trf!(
            "test {0} {0} {number:.1}",
            {
                calls += 1;
                &value
            },
            number = 1.25
        );
        assert_eq!(calls, 1);
        assert_eq!(value, "unchanged {name}.txt");
        assert_eq!(actual, "test unchanged {name}.txt unchanged {name}.txt 1.2");
    }
}

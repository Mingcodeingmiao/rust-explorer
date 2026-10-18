//! In-memory column filters. Entry indices remain stable in the owning directory.
use crate::fs::Entry;
use chrono::{DateTime, Datelike, Local, NaiveDate};
use pinyin::ToPinyin;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::SystemTime,
};

#[derive(Clone, Debug, Default)]
pub struct Filters {
    pub columns: BTreeMap<String, BTreeSet<String>>,
}

pub const NAMES: &[(&str, &str)] = &[
    ("digits", "0–9"),
    ("latin-ah", "A–H"),
    ("latin-ip", "I–P"),
    ("latin-qz", "Q–Z"),
    ("pinyin-af", "拼音 A–F"),
    ("pinyin-gl", "拼音 G–L"),
    ("pinyin-ms", "拼音 M–S"),
    ("pinyin-tz", "拼音 T–Z"),
    ("other", "其他"),
];
pub const DATES: &[(&str, &str)] = &[
    ("old", "很久以前"),
    ("year", "今年早些时候"),
    ("last-month", "上月"),
    ("month", "本月早些时候"),
];
pub const SIZES: &[(&str, &str)] = &[
    ("tiny", "0–16 KB"),
    ("small", "16 KB–1 MB"),
    ("medium", "1–128 MB"),
    ("large", "更大"),
    ("unknown", "未指定"),
];

/// Select a range in rendered order, never raw Entry index order.
pub fn selection_range(order: &[usize], anchor: usize, target: usize) -> Vec<usize> {
    let Some(end) = order.iter().position(|i| *i == target) else {
        return Vec::new();
    };
    let start = order.iter().position(|i| *i == anchor).unwrap_or(end);
    order[start.min(end)..=start.max(end)].to_vec()
}
/// Advance through visible files only; missing focus starts at the first file.
pub fn step_in_order(order: &[usize], current: Option<usize>, delta: isize) -> Option<usize> {
    if order.is_empty() {
        return None;
    }
    let index = current.and_then(|current| order.iter().position(|i| *i == current));
    let target = index
        .map(|index| (index as isize + delta).clamp(0, order.len() as isize - 1) as usize)
        .unwrap_or(0);
    Some(order[target])
}

pub fn name_bucket(name: &str) -> &'static str {
    let Some(first) = name.chars().next() else {
        return "other";
    };
    match first.to_ascii_uppercase() {
        '0'..='9' => "digits",
        'A'..='H' => "latin-ah",
        'I'..='P' => "latin-ip",
        'Q'..='Z' => "latin-qz",
        _ => match first
            .to_pinyin()
            .and_then(|p| p.plain().chars().next())
            .map(|c| c.to_ascii_uppercase())
        {
            Some('A'..='F') => "pinyin-af",
            Some('G'..='L') => "pinyin-gl",
            Some('M'..='S') => "pinyin-ms",
            Some('T'..='Z') => "pinyin-tz",
            _ => "other",
        },
    }
}

pub fn size_bucket(size: Option<u64>) -> &'static str {
    match size {
        None => "unknown",
        Some(v) if v <= 16 * 1024 => "tiny",
        Some(v) if v <= 1024 * 1024 => "small",
        Some(v) if v <= 128 * 1024 * 1024 => "medium",
        _ => "large",
    }
}

pub fn date_bucket(date: Option<SystemTime>, today: NaiveDate) -> &'static str {
    let Some(date) = date.map(|d| DateTime::<Local>::from(d).date_naive()) else {
        return "unknown";
    };
    date_bucket_naive(date, today)
}
fn date_bucket_naive(date: NaiveDate, today: NaiveDate) -> &'static str {
    let month = today.with_day(1).unwrap();
    let previous = month.pred_opt().unwrap().with_day(1).unwrap();
    let year = today.with_month(1).unwrap().with_day(1).unwrap();
    if date >= previous && date < month {
        "last-month"
    } else if date >= month && date < today {
        "month"
    } else if date >= year && date < previous {
        "year"
    } else if date < year && date < previous {
        "old"
    } else {
        "recent"
    }
}
impl Filters {
    pub fn active(&self, column: &str) -> bool {
        self.columns.get(column).is_some_and(|s| !s.is_empty())
    }
    pub fn checked(&self, column: &str, option: &str) -> bool {
        self.columns.get(column).is_some_and(|s| s.contains(option))
    }
    pub fn toggle(&mut self, column: &str, option: &str) {
        let set = self.columns.entry(column.to_owned()).or_default();
        if !set.remove(option) {
            set.insert(option.to_owned());
        }
        if set.is_empty() {
            self.columns.remove(column);
        }
    }
    pub fn matches(&self, entry: &Entry, folder_size: Option<u64>, today: NaiveDate) -> bool {
        self.columns.iter().all(|(column, options)| {
            if options.is_empty() {
                return true;
            }
            let key = match column.as_str() {
                "name" => name_bucket(&entry.name),
                "kind" => &entry.kind,
                "modified" => date_bucket(entry.modified, today),
                "created" => date_bucket(entry.created, today),
                "size" => size_bucket(if entry.directory {
                    folder_size
                } else {
                    Some(entry.size)
                }),
                _ => return true,
            };
            options.contains(key)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(name: &str, kind: &str, size: u64) -> Entry {
        Entry {
            path: name.into(),
            name: name.into(),
            directory: false,
            link: false,
            hidden: false,
            system: false,
            size,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: kind.into(),
        }
    }
    #[test]
    fn pinyin_and_latin_ranges_are_distinct_and_complete() {
        for (name, bucket) in [
            ("3文件", "digits"),
            ("Apple", "latin-ah"),
            ("hello", "latin-ah"),
            ("Java", "latin-ip"),
            ("zed", "latin-qz"),
            ("安徽", "pinyin-af"),
            ("广州", "pinyin-gl"),
            ("上海", "pinyin-ms"),
            ("武汉", "pinyin-tz"),
            ("_file", "other"),
        ] {
            assert_eq!(name_bucket(name), bucket, "{name}");
        }
    }
    #[test]
    fn column_or_combines_with_cross_column_and_without_reindexing() {
        let files = [
            entry("A.png", "PNG", 15),
            entry("B.txt", "TXT", 20),
            entry("I.png", "PNG", 30),
            entry("Z.png", "PNG", 40),
        ];
        let mut filters = Filters::default();
        filters.toggle("name", "latin-ah");
        filters.toggle("name", "latin-ip");
        filters.toggle("kind", "PNG");
        let matches: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, e)| filters.matches(e, None, Local::now().date_naive()))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(matches, [0, 2]);
        filters.toggle("kind", "PNG");
        assert!(!filters.active("kind"));
    }
    #[test]
    fn size_boundaries_do_not_overlap_and_unspecified_is_not_zero() {
        assert_eq!(size_bucket(None), "unknown");
        assert_eq!(size_bucket(Some(0)), "tiny");
        assert_eq!(size_bucket(Some(16 * 1024)), "tiny");
        assert_eq!(size_bucket(Some(16 * 1024 + 1)), "small");
        assert_eq!(size_bucket(Some(1024 * 1024)), "small");
        assert_eq!(size_bucket(Some(1024 * 1024 + 1)), "medium");
        assert_eq!(size_bucket(Some(128 * 1024 * 1024 + 1)), "large");
    }
    #[test]
    fn calendar_ranges_handle_january_and_month_ends() {
        let day = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let now = day(2026, 1, 17);
        assert_eq!(date_bucket_naive(day(2025, 12, 31), now), "last-month");
        assert_eq!(date_bucket_naive(day(2025, 11, 30), now), "old");
        assert_eq!(date_bucket_naive(day(2026, 1, 1), now), "month");
        assert_eq!(date_bucket_naive(now, now), "recent");
        assert_eq!(
            date_bucket_naive(day(2026, 3, 31), day(2026, 10, 17)),
            "year"
        );
    }
    #[test]
    fn navigation_and_shift_ranges_follow_visible_group_order() {
        let order = [8, 2, 11, 5]; // Filtering and grouping changed visual order.
        assert_eq!(step_in_order(&order, Some(2), 1), Some(11));
        assert_eq!(step_in_order(&order, Some(2), -1), Some(8));
        assert_eq!(step_in_order(&order, Some(99), 1), Some(8));
        assert_eq!(step_in_order(&order, Some(5), 50), Some(5));
        assert_eq!(step_in_order(&[], None, 1), None);
        assert_eq!(selection_range(&order, 2, 5), [2, 11, 5]);
        assert_eq!(selection_range(&order, 5, 2), [2, 11, 5]);
        assert_eq!(selection_range(&order, 99, 2), [2]);
        assert!(selection_range(&order, 8, 99).is_empty());
    }
}

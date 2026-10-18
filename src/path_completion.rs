//! Small, synchronous path completion for the address bar. The caller limits
//! the result count before rendering, so this never walks a directory tree.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}

/// Split what the user typed into the directory to enumerate and the name
/// prefix to filter. Relative input is resolved against `current`.
pub fn split_input(current: &Path, input: &str) -> Option<(PathBuf, String)> {
    let input = input.trim().trim_matches('"');
    if input.is_empty() || input.starts_with("::") || input.starts_with("shell:") {
        return None;
    }
    let input = expand_environment(input);
    if input.len() == 2 && input.as_bytes()[1] == b':' {
        return Some((PathBuf::from(format!("{input}\\")), String::new()));
    }
    let path = PathBuf::from(&input);
    let path = if path.is_relative() {
        current.join(&path)
    } else {
        path
    };
    let ends_separator = input.ends_with('\\') || input.ends_with('/');
    let (directory, prefix) = if ends_separator {
        (path, String::new())
    } else {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => {
                (parent.to_path_buf(), name.to_string_lossy().to_string())
            }
            _ => (current.to_path_buf(), input.clone()),
        }
    };
    Some((directory, prefix))
}

fn expand_environment(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('%') {
        result.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else {
            result.push_str(&rest[start..]);
            return result;
        };
        let name = &after[..end];
        if name.is_empty() {
            // A pair of percent signs is a literal percent sign.
            result.push('%');
        } else {
            match std::env::var(name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    result.push('%');
                    result.push_str(name);
                    result.push('%');
                }
            }
        }
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    result
}

/// Enumerate at most `limit` matching children. Directories sort first.
pub fn complete(current: &Path, input: &str, limit: usize) -> Vec<Suggestion> {
    let Some((directory, prefix)) = split_input(current, input) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_lowercase();
    let mut suggestions = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "." || name == ".." {
            continue;
        }
        if !prefix_lower.is_empty() && !name.to_lowercase().starts_with(&prefix_lower) {
            continue;
        }
        let directory_entry = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        suggestions.push(Suggestion {
            path: entry.path(),
            name,
            directory: directory_entry,
        });
    }
    suggestions.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| crate::fs::natural_cmp(&a.name, &b.name))
    });
    suggestions.truncate(limit);
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_absolute_and_relative_inputs() {
        let current = Path::new("D:\\work");
        assert_eq!(
            split_input(current, "C:\\Windows\\Sys"),
            Some((PathBuf::from("C:\\Windows"), "Sys".into()))
        );
        assert_eq!(
            split_input(current, "C:\\Windows\\"),
            Some((PathBuf::from("C:\\Windows\\"), String::new()))
        );
        assert_eq!(
            split_input(current, "src\\ma"),
            Some((PathBuf::from("D:\\work\\src"), "ma".into()))
        );
        assert_eq!(
            split_input(current, "C:"),
            Some((PathBuf::from("C:\\"), String::new()))
        );
        assert_eq!(split_input(current, "::ThisPC"), None);
    }

    #[test]
    fn expands_environment_names_without_touching_missing_variables() {
        assert_eq!(expand_environment("100%%"), "100%");
        assert_eq!(
            expand_environment("%__MISSING_DSH_VALUE__%"),
            "%__MISSING_DSH_VALUE__%"
        );
    }
}

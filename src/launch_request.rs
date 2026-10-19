//! Explicit folder opening / reveal requests from other programs.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchRequest {
    pub folder: PathBuf,
    pub select: Vec<PathBuf>,
}

fn canonical(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("找不到项目：{}", path.display()))?;
    let text = path.to_string_lossy();
    Ok(if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        text.strip_prefix(r"\\?\")
            .map(PathBuf::from)
            .unwrap_or(path)
    })
}
pub fn parse(args: &[OsString]) -> Result<Option<LaunchRequest>> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    let text = first.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    let (reveal, targets): (bool, Vec<OsString>) = if matches!(
        lower.as_str(),
        "--select" | "/select" | "/select," | "-select"
    ) {
        (true, args[1..].to_vec())
    } else if lower.starts_with("/select,") {
        let mut values = vec![OsString::from(&text[8..])];
        values.extend_from_slice(&args[1..]);
        (true, values)
    } else if matches!(lower.as_str(), "--open-folder" | "--browse-folder") {
        (false, args[1..].to_vec())
    } else if !text.starts_with('-') && Path::new(first).exists() {
        (false, vec![first.clone()])
    } else {
        return Ok(None);
    };
    ensure!(!targets.is_empty(), "缺少要打开或定位的项目路径");
    let mut selected = vec![];
    let mut folder = None;
    for target in targets {
        let text = target.to_string_lossy();
        let path = canonical(Path::new(text.trim_matches('"')))?;
        let parent = if reveal || path.is_file() {
            selected.push(path.clone());
            path.parent().context("无法定位根目录本身")?.to_path_buf()
        } else {
            ensure!(path.is_dir(), "目标不是文件夹");
            path
        };
        ensure!(
            folder.as_ref().is_none_or(|f| f == &parent),
            "一次定位的项目必须位于同一文件夹"
        );
        folder = Some(parent);
    }
    Ok(Some(LaunchRequest {
        folder: folder.unwrap(),
        select: selected,
    }))
}
pub fn current() -> Result<Option<LaunchRequest>> {
    parse(&std::env::args_os().skip(1).collect::<Vec<_>>())
}

/// A different file must not replace a tab's existing reveal target.
pub fn find_reveal_tab<'a>(
    request: &LaunchRequest,
    tabs: impl Iterator<Item = (usize, &'a Path, &'a [PathBuf])>,
) -> Option<usize> {
    let mut empty = None;
    for (index, folder, targets) in tabs {
        if folder != request.folder {
            continue;
        }
        if request.select.is_empty()
            || (targets.len() == request.select.len()
                && request.select.iter().all(|path| targets.contains(path)))
        {
            return Some(index);
        }
        if targets.is_empty() && empty.is_none() {
            empty = Some(index);
        }
    }
    empty
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_reveal_keeps_different_files_in_separate_tabs() {
        let folder = PathBuf::from(r"C:\reveal");
        let a = folder.join("a.txt");
        let b = folder.join("b.txt");
        let mut tabs = vec![(folder.clone(), vec![a.clone()])];
        let request = LaunchRequest {
            folder: folder.clone(),
            select: vec![b.clone()],
        };
        let find = |tabs: &Vec<(PathBuf, Vec<PathBuf>)>, request: &LaunchRequest| {
            find_reveal_tab(
                request,
                tabs.iter()
                    .enumerate()
                    .map(|(i, (p, t))| (i, p.as_path(), t.as_slice())),
            )
        };
        assert_eq!(find(&tabs, &request), None);
        tabs.push((folder.clone(), vec![b]));
        assert_eq!(find(&tabs, &request), Some(1));
        assert_eq!(
            find(
                &tabs,
                &LaunchRequest {
                    folder: folder.clone(),
                    select: vec![a]
                }
            ),
            Some(0)
        );
        assert_eq!(
            find(
                &tabs,
                &LaunchRequest {
                    folder: folder.clone(),
                    select: vec![]
                }
            ),
            Some(0)
        );
        tabs.push((folder.clone(), vec![]));
        assert_eq!(
            find(
                &tabs,
                &LaunchRequest {
                    folder: folder.clone(),
                    select: vec![folder.join("c.txt")]
                }
            ),
            Some(2)
        );
    }
    #[test]
    fn reveal_supports_explorer_syntax_unicode_commas_and_multiple_targets() {
        let dir = std::env::temp_dir().join(format!("re-reveal-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let a = dir.join("中文 空格,文件.txt");
        let b = dir.join("second.txt");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();
        for args in [
            vec!["--select".into(), a.clone().into_os_string()],
            vec![format!("/select,{}", a.display()).into()],
            vec!["/select,".into(), a.clone().into_os_string()],
        ] {
            let request = parse(&args).unwrap().unwrap();
            assert_eq!(request.folder, canonical(&dir).unwrap());
            assert_eq!(request.select, vec![canonical(&a).unwrap()]);
        }
        let request = parse(&[
            "--select".into(),
            a.clone().into_os_string(),
            b.clone().into_os_string(),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(request.select.len(), 2);
        let request = parse(&["--open-folder".into(), a.clone().into_os_string()])
            .unwrap()
            .unwrap();
        assert_eq!(request.select.len(), 1);
        assert!(parse(&["--select".into()]).is_err());
        assert!(parse(&["--shell".into()]).unwrap().is_none());
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}

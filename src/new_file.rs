use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
#[derive(Clone)]
pub struct Template {
    pub name: String,
    pub extension: String,
    pub data: Vec<u8>,
}
pub fn templates() -> Vec<Template> {
    let mut out = vec![Template {
        name: "文本文档".into(),
        extension: ".txt".into(),
        data: vec![],
    }];
    if let Ok(keys) = windows_registry::CLASSES_ROOT.keys() {
        for extension in keys.filter(|s| s.starts_with('.')) {
            if extension == ".txt" || extension.len() > 24 {
                continue;
            }
            let Ok(key) = windows_registry::CLASSES_ROOT.open(format!("{extension}\\ShellNew"))
            else {
                continue;
            };
            let data = if key.get_type("NullFile").is_ok() {
                vec![]
            } else if let Ok(data) = key.get_value("Data") {
                data.to_vec()
            } else if let Ok(file) = key.get_string("FileName") {
                let path = PathBuf::from(&file);
                let windir = std::env::var_os("WINDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
                let Some(data) = [path, windir.join("ShellNew").join(&file)]
                    .iter()
                    .find_map(|p| {
                        std::fs::metadata(p)
                            .ok()
                            .filter(|m| m.len() <= 4 * 1024 * 1024)
                            .and_then(|_| std::fs::read(p).ok())
                    })
                else {
                    continue;
                };
                data
            } else {
                continue;
            };
            let name = windows_registry::CLASSES_ROOT
                .open(&extension)
                .and_then(|k| k.get_string(""))
                .ok()
                .and_then(|class| {
                    windows_registry::CLASSES_ROOT
                        .open(class)
                        .and_then(|k| k.get_string(""))
                        .ok()
                })
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    format!("{} 文档", extension.trim_start_matches('.').to_uppercase())
                });
            out.push(Template {
                name,
                extension,
                data,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(24);
    out
}
pub fn create(folder: &Path, template: &Template) -> Result<PathBuf> {
    for i in 0..10000 {
        let name = if i == 0 {
            format!("新建{}{}", template.name, template.extension)
        } else {
            format!("新建{} ({i}){}", template.name, template.extension)
        };
        let path = folder.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(&template.data)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("新建文档"),
        }
    }
    bail!("无法生成不重复的名称")
}

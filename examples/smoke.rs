use anyhow::Result;
use rust_explorer::{fs, preview, search, shell};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tools/smoke-workspace");
    std::fs::create_dir_all(&root)?;
    let run = root.join(format!("run-{}", std::process::id()));
    std::fs::create_dir(&run)?;
    let source = run.join("中文 空格.txt");
    std::fs::write(&source, "文本预览与文件复制测试\nline 2")?;
    let destination = run.join("destination");
    std::fs::create_dir(&destination)?;
    shell::dispatch(shell::Request::Copy {
        paths: vec![source.clone()],
        destination: destination.clone(),
        moving: false,
    })?;
    assert_eq!(
        std::fs::read(&source)?,
        std::fs::read(destination.join("中文 空格.txt"))?
    );
    shell::dispatch(shell::Request::Rename {
        path: destination.join("中文 空格.txt"),
        name: "已重命名.txt".into(),
    })?;
    assert!(destination.join("已重命名.txt").is_file());
    let moved = run.join("moved");
    std::fs::create_dir(&moved)?;
    shell::dispatch(shell::Request::Copy {
        paths: vec![destination.join("已重命名.txt")],
        destination: moved.clone(),
        moving: true,
    })?;
    assert!(!destination.join("已重命名.txt").exists());
    assert!(moved.join("已重命名.txt").is_file());
    let text = preview::text(&source)?;
    assert_eq!(text.lines.len(), 2);
    let utf16 = run.join("utf16.txt");
    let mut bytes = vec![0xff, 0xfe];
    for n in "你好🌍\n第二行".encode_utf16() {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    std::fs::write(&utf16, bytes)?;
    assert_eq!(preview::text(&utf16)?.lines[0], "你好🌍");
    let mut entries = Vec::new();
    for i in (0..100_000).rev() {
        entries.push(fs::Entry {
            recycle: None,
            path: PathBuf::from(format!("{i}.txt")),
            name: format!("文件{i}.txt"),
            directory: false,
            hidden: false,
            link: false,
            system: false,
            size: i,
            total: None,
            free: None,
            modified: None,
            created: None,
            kind: "TXT 文件".into(),
        });
    }
    let start = Instant::now();
    fs::sort(&mut entries, fs::Sort::Name, false);
    let elapsed = start.elapsed();
    assert_eq!(entries.first().unwrap().name, "文件0.txt");
    assert_eq!(entries.last().unwrap().name, "文件99999.txt");
    let cancel = Arc::new(AtomicBool::new(false));
    let (items, _) = fs::read_directory(&run, &cancel)?;
    assert_eq!(items.len(), 4);
    let everything = if search::available() {
        let result = search::sdk_query(&search::scoped_query(
            "ext:toml",
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            false,
        ))?;
        assert!(result.entries.iter().any(|e| e.name == "Cargo.toml"));
        format!("{} results", result.total)
    } else {
        "unavailable".into()
    };
    println!("Shell copy/rename/move, UTF-8/UTF-16 previews, directory enumeration: PASS");
    println!(
        "100,000-entry natural sort: {} ms; Everything: {everything}",
        elapsed.as_millis()
    );
    println!("Isolated fixtures retained at {}", run.display());
    Ok(())
}

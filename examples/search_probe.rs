fn main() -> anyhow::Result<()> {
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    rust_explorer::search::ensure_running(&cancel)?;
    println!("Everything SDK: {:?}", rust_explorer::search::sdk_path());
    println!(
        "Everything IPC available: {}",
        rust_explorer::search::available()
    );
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Cargo.toml".into());
    let result = rust_explorer::search::sdk_query(&query)?;
    println!(
        "{} results, {} returned",
        result.total,
        result.entries.len()
    );
    Ok(())
}

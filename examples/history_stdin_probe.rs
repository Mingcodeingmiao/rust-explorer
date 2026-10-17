//! Owned-fixture regression harness for the actual parent/child Shell protocol.
fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--shell-stdin")
        && std::env::var_os("RUST_EXPLORER_OWNED_PIPE_STRESS").is_some()
    {
        use std::io::Write;
        std::io::stdout().write_all(&vec![b' '; 10 * 1024 * 1024])?;
        std::io::stdout().write_all(b"\n")?;
        std::io::stderr().write_all(&vec![b' '; 256 * 1024])?;
    }
    let request = rust_explorer::shell::read_request(std::io::stdin().lock())?;
    if std::env::args().nth(1).as_deref() == Some("--shell-stdin") {
        rust_explorer::shell::dispatch(request)
    } else {
        println!("{}", rust_explorer::shell::execute_output(request)?);
        Ok(())
    }
}

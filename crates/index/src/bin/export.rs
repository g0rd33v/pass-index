//! Dump the catalogue as JSON on stdout.

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "index.db".into());
    println!("{}", index::Index::open(&path)?.export_json()?);
    Ok(())
}

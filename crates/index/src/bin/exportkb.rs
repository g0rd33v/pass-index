//! The catalogue as documents, for a knowledge base to read.
//!
//!   exportkb <db> [--out <dir>]

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db = args.get(1).cloned().unwrap_or_else(|| "/data/index.db".into());
    let out = args.iter().position(|a| a == "--out")
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| "/tmp/kb".into());
    let con = common::db::open(&db)?;
    index::prepare(&con)?;
    index::hands::export_kb(&con, &out)?;
    Ok(())
}

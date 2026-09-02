//! What a reader is actually handed, walked over HTTP.
//!
//!   audit [--base http://127.0.0.1:8185] [--db <path>] [--workers 8]
//!
//! Exit 1 if anything blocks. The verdict is recorded where the status page
//! can read it, so that page shows the findings that would have stopped the
//! nightly run rather than a claim about them.

use anyhow::Result;
use index::{checks, walk};

#[tokio::main]
async fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let arg = |name: &str, fallback: &str| -> String {
        a.iter()
            .position(|x| x == name)
            .and_then(|i| a.get(i + 1).cloned())
            .unwrap_or_else(|| fallback.into())
    };
    let base = arg("--base", "http://127.0.0.1:8185");
    let db = arg("--db", "/srv/pass-index/data/index.db");
    let workers: usize = arg("--workers", "8").parse().unwrap_or(8);

    let (_walked, blocked, f) = walk::walk(&base, workers).await?;
    let con = common::db::open(&db)?;
    index::prepare(&con)?;
    let verdicts: Vec<checks::Verdict> = f
        .verdicts()
        .into_iter()
        .map(|(name, blocking, findings, asks)| checks::Verdict { name, blocking, findings, asks })
        .collect();
    checks::record(&con, "pages", &verdicts)?;
    std::process::exit(if blocked > 0 { 1 } else { 0 });
}

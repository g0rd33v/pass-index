//! What must hold for the catalogue to be worth reading.
//!
//!   check <db>
//!
//! Exit 1 if anything blocks, 0 otherwise — so a nightly run that finds the
//! catalogue asserting something false stops and says so.

use anyhow::Result;
use index::checks;

fn main() -> Result<()> {
    let db = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/srv/pass-index/data/index.db".into());
    let con = common::db::open(&db)?;
    index::prepare(&con)?;
    let (blocked, _warned, verdicts) = checks::run(&con, &db)?;
    checks::record(&con, "database", &verdicts)?;
    std::process::exit(if blocked > 0 { 1 } else { 0 });
}

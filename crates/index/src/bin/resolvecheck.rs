//! Does the Rust resolver bind exactly as the Python one did?
//!
//! A rewrite of the resolver is the one change that can go wrong silently:
//! everything still runs, and prices land on different models. So the port is
//! not trusted until it has been asked the same questions as the original and
//! given the same answers — every alias in the catalogue, and every name the
//! feeds have offered us.
//!
//!     resolvecheck <db> [names-file]
//!
//! Prints one line per name whose binding differs from the answer supplied on
//! stdin as `name<TAB>entity_id`, and a count.
use anyhow::Result;
use index::resolve::Resolver;
use std::io::{BufRead, Write};

fn main() -> Result<()> {
    let db = std::env::args().nth(1).unwrap_or_else(|| "/data/index.db".into());
    let ix = index::Index::open(&db)?;
    let r = Resolver::build(&ix)?;
    let stdin = std::io::stdin();
    // --dump: one `name<TAB>binding` line per stdin name, nothing compared.
    // This is how a change to the resolver is measured: dump before, dump
    // after, and every line that moved is a price moving between cards.
    if std::env::args().any(|a| a == "--dump") {
        let out = std::io::stdout();
        let mut out = out.lock();
        for line in stdin.lock().lines() {
            let name = line?;
            writeln!(out, "{name}	{}", r.look(&name).unwrap_or_default())?;
        }
        return Ok(());
    }
    let mut same = 0usize;
    let mut differ = 0usize;
    let out = std::io::stdout();
    let mut out = out.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let mut parts = line.splitn(2, '\t');
        let name = parts.next().unwrap_or("");
        let want = parts.next().unwrap_or("");
        let got = r.look(name).unwrap_or_default();
        if got == want {
            same += 1;
        } else {
            differ += 1;
            if differ <= 500 {
                writeln!(out, "DIFFER  {name}\n  python: {want}\n  rust  : {got}")?;
            }
        }
    }
    writeln!(out, "\nsame: {same}, differ: {differ}")?;
    Ok(())
}

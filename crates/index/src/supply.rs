//! The standard door for an outside supplier of findings.
//!
//! The crawler is the first supplier, and this module is written as the
//! contract every later one signs. A supplier delivers dated JSONL files,
//! one observation per line, each carrying at least:
//!
//!   what        mention | price | standing | event | watch | snapshot
//!   name        the subject as the source wrote it
//!   says        the evidence, quoted
//!   source      the URL the evidence stands on
//!   source_id   which of the supplier's readers produced it
//!   read_at     when
//!
//! and optionally maker, seller, lane, unit, confidence.
//!
//! Five rules, none negotiable:
//!
//!   1. A supplier never mints identity. A name that does not bind through
//!      the resolver goes to the pen as a candidate, or is counted and
//!      dropped; it never becomes an entity here.
//!   2. Every figure that lands carries its source and its date, and lands
//!      through the same door as every other price — the resolver-bound
//!      write path — never through private INSERTs.
//!   3. A figure the catalogue cannot state exactly is kept as evidence, not
//!      as a price. "¥12 per thousand tokens" is a true quote and a false
//!      dollar rate; it lands in `docs` as a quoted line with its source,
//!      and no arithmetic is invented to promote it.
//!   4. Every file is processed once. The file's hash is remembered, and a
//!      re-run over unchanged files writes nothing.
//!   5. Every run is recorded — read, bound, written, dropped — so the
//!      coverage page shows the supplier's health next to every other
//!      source, and a supplier that goes quiet is visible.

use crate::feed;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;

const SEEN_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS supply_seen (
    file    TEXT PRIMARY KEY,
    hash    TEXT NOT NULL,
    done_at TEXT NOT NULL
);";

pub struct Tally {
    pub files: usize,
    pub skipped_files: usize,
    pub read: usize,
    pub prices: usize,
    pub evidence: usize,
    pub standings: usize,
    pub candidates: usize,
    pub dropped: usize,
}

fn file_fingerprint(bytes: &[u8]) -> String {
    // A change detector, not a checksum: two folded FNV-64 passes, enough to
    // notice a findings file that differs from the one already consumed.
    let mut a: u64 = 0xcbf29ce484222325;
    let mut b: u64 = 0x9e3779b97f4a7c15;
    for &x in bytes {
        a = (a ^ x as u64).wrapping_mul(0x100000001b3);
        b = b.rotate_left(7) ^ a;
    }
    format!("{a:016x}{b:016x}")
}

/// "$18,000 per year", "$1/1k req", "+$184K / mo", "$0.25 per 1M tokens" —
/// the shapes a hunted pricing page states in dollars. Anything this cannot
/// reduce to a dimension the catalogue already speaks stays evidence.
fn parse_price(says: &str) -> Option<(&'static str, i64)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(
        r"(?i)\$\s?([\d][\d,]*(?:\.\d+)?)\s*([kKmM]\b)?\s*(?:/|per\s+)?\s*(mo\b|month|1M tokens|million tokens|1k req|1000 req|call|request|year|yr\b|annual)",
    ).expect("price regex"));
    let c = re.captures(says)?;
    let mut amount: f64 = c.get(1)?.as_str().replace(',', "").parse().ok()?;
    match c.get(2).map(|m| m.as_str().to_lowercase()) {
        Some(k) if k == "k" => amount *= 1e3,
        Some(m) if m == "m" => amount *= 1e6,
        _ => {}
    }
    let unit = c.get(3)?.as_str().to_lowercase();
    // Only arithmetic that changes no meaning: a yearly figure is not made
    // monthly, because "starting at $18,000 a year" is a floor, not a rate.
    match unit.as_str() {
        "mo" | "month" => Some(("month", (amount * 1e6) as i64)),
        "1m tokens" | "million tokens" => Some(("mtok_in", (amount * 1e6) as i64)),
        "1k req" | "1000 req" => Some(("call", (amount / 1000.0 * 1e6) as i64)),
        "call" | "request" => Some(("call", (amount * 1e6) as i64)),
        _ => None,
    }
}

/// "1 of 105 on Vectara Hallucination Leaderboard — Hallucination Rate 1.8 %"
fn parse_standing(says: &str) -> Option<(i64, i64, String, String, f64)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(
        r"^(\d+) of (\d+) on (.+?) — (.+?) ([\d.]+)\s*%?\s*$",
    ).expect("standing regex"));
    let c = re.captures(says)?;
    Some((
        c.get(1)?.as_str().parse().ok()?,
        c.get(2)?.as_str().parse().ok()?,
        c.get(3)?.as_str().trim().to_string(),
        c.get(4)?.as_str().trim().to_string(),
        c.get(5)?.as_str().parse().ok()?,
    ))
}

/// Whether a lower number is the better result on a board, read from its
/// metric name. A rate, an error, a latency, a cost, a perplexity — lower is
/// better; a score or accuracy — higher. Hardcoding 0 published a
/// hallucination-rate board as "higher is better".
fn lower_is_better(metric: &str) -> bool {
    let m = metric.to_lowercase();
    ["rate", "error", "latency", "cost", "price", "perplex", "wer", "loss",
     "hallucinat"]
        .iter()
        .any(|w| m.contains(w))
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

/// One evidence line on a bound subject: the quote, its source, its day.
///
/// The docs UNIQUE key is (subject, kind, field, source_url), so two distinct
/// quotes from one URL — an "event" line and a "snapshot" line about the same
/// model — would collide and the second would be silently dropped. The quote's
/// own fingerprint goes into `field`, so each distinct quote from a page keeps
/// its own row while a re-run of the identical quote stays idempotent.
fn evidence(con: &Connection, subject: &str, says: &str, source: &str, day: &str) -> Result<bool> {
    let field = format!("q{}", file_fingerprint(says.as_bytes()));
    let n = con.execute(
        "INSERT OR IGNORE INTO docs (subject, kind, field, text, source_url, taken_at) \
         VALUES (?1, 'evidence', ?2, ?3, ?4, ?5)",
        rusqlite::params![subject, field, says, source, day],
    )?;
    Ok(n > 0)
}

pub fn consume(
    con: &Connection,
    pen: Option<&Connection>,
    dir: &str,
    apply: bool,
) -> Result<Tally> {
    con.execute_batch(SEEN_SCHEMA)?;
    let mut r = crate::resolve::Resolver::from_conn(con)?;
    // The delivery is dated by its own findings' read_at, not by "now"
    // (MAX(prices.taken_at)): a file delivered late — a mount reconnecting
    // with a three-week-old crawl — must carry its real date, or it would be
    // stamped today and win the seller-fresh tie against the correct current
    // price. The newest read_at across the lines is the delivery's date;
    // the catalogue's own latest date is only the fallback when a file
    // carries no read_at at all.
    let prices_max: String = con.query_row("SELECT MAX(taken_at) FROM prices", [], |x| x.get(0))?;
    let mut delivery_date: Option<String> = None;

    // The providers the catalogue holds, keyed by the form names are
    // compared in. Two uses: the mention filter (a maker we already list
    // publishing something new is what the pen is for), and turning a
    // supplier's seller string into an id that EXISTS — a supplier never
    // mints identity, so a seller we cannot place drops rather than creating
    // a ghost provider that carries a price no page can show.
    let mut seller_id: HashMap<String, String> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers")?;
        let rows: Vec<(String, String)> =
            q.query_map([], |x| Ok((x.get(0)?, x.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (id, name) in rows {
            seller_id.insert(crate::resolve::norm(&name), id.clone());
            seller_id.insert(crate::resolve::norm(&id), id);
        }
    }

    let mut t = Tally {
        files: 0, skipped_files: 0, read: 0, prices: 0,
        evidence: 0, standings: 0, candidates: 0, dropped: 0,
    };
    let mut price_obs: Vec<feed::Observation> = Vec::new();
    let mut done_files: Vec<(String, String)> = Vec::new();
    let mut standings: Vec<(String, String, String, f64, i64, i64, String, String)> = Vec::new();
    // Nothing touches the database during the scan; every write is collected
    // here and applied in the one transaction at the end, so a dry run
    // writes nothing and a crashed apply leaves no half-delivery behind.
    let mut evidence_rows: Vec<(String, String, String)> = Vec::new();
    let mut pen_rows: Vec<(String, String, String)> = Vec::new();

    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "jsonl").unwrap_or(false))
        .collect();
    files.sort();

    for path in files {
        let body = std::fs::read(&path)?;
        let hash = file_fingerprint(&body);
        let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let done: Option<String> = con
            .query_row("SELECT hash FROM supply_seen WHERE file=?1", [&fname], |x| x.get(0))
            .ok();
        if done.as_deref() == Some(hash.as_str()) {
            t.skipped_files += 1;
            continue;
        }
        t.files += 1;

        for line in String::from_utf8_lossy(&body).lines() {
            let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
            t.read += 1;
            let what = d["what"].as_str().unwrap_or("");
            let name = d["name"].as_str().unwrap_or("");
            let says = d["says"].as_str().unwrap_or("");
            let source = d["source"].as_str().unwrap_or("");
            if let Some(read_at) = d["read_at"].as_str() {
                let date: String = read_at.chars().take(10).collect();
                if date.len() == 10 && delivery_date.as_deref().map(|d| date.as_str() > d).unwrap_or(true) {
                    delivery_date = Some(date);
                }
            }
            if name.is_empty() || source.is_empty() {
                t.dropped += 1;
                continue;
            }
            match what {
                "price" => {
                    let seller = d["seller"].as_str().unwrap_or("");
                    let known_seller = seller_id.get(&crate::resolve::norm(seller)).cloned();
                    match (parse_price(says), r.look(name), known_seller) {
                        // A figure the catalogue can state exactly, from a
                        // seller it already holds: a real price.
                        (Some((dim, micros)), Some(_), Some(pid)) => {
                            price_obs.push(feed::Observation {
                                subject: name.to_string(),
                                seller: pid,
                                payload: vec![(dim.to_string(), micros)],
                                source_url: source.to_string(),
                            });
                            t.prices += 1;
                        }
                        // Bound thing, but the figure will not reduce or the
                        // seller is a stranger: keep the quote as evidence.
                        (_, Some(eid), _) => {
                            evidence_rows.push((eid, says.to_string(), source.to_string()));
                            t.evidence += 1;
                        }
                        (_, None, _) => t.dropped += 1,
                    }
                }
                "standing" => match (parse_standing(says), r.look(name)) {
                    (Some((rank, out_of, board, metric, value)), Some(eid)) => {
                        standings.push((
                            eid,
                            format!("crawler_{}", slug(&board)),
                            board, value, rank, out_of, metric,
                            source.to_string(),
                        ));
                        t.standings += 1;
                    }
                    _ => t.dropped += 1,
                },
                "mention" => {
                    let maker = d["maker"].as_str().unwrap_or("");
                    let known = !maker.is_empty()
                        && seller_id.contains_key(&crate::resolve::norm(maker));
                    if known && r.look(name).is_none() && pen.is_some() {
                        // The same id scheme feed::mint_id uses, so a
                        // candidate the pen holds is the id newmodels will
                        // compute for the same model — the pen's refusal
                        // actually suppresses the re-mint. `slug` (underscores)
                        // never matched mint_id (hyphens), so it did not.
                        pen_rows.push((feed::mint_id(name), name.to_string(), maker.to_string()));
                        t.candidates += 1;
                    } else if !known {
                        t.dropped += 1;
                    }
                }
                // An event or a snapshot about something we hold is a dated
                // quote worth keeping beside the subject.
                "event" | "watch" | "snapshot" => match r.look(name) {
                    Some(eid) => {
                        evidence_rows.push((eid, says.to_string(), source.to_string()));
                        t.evidence += 1;
                    }
                    None => t.dropped += 1,
                },
                _ => t.dropped += 1,
            }
        }
        if apply {
            done_files.push((fname, hash));
        }
    }

    // One transaction for the whole delivery. A run that dies half-way must
    // leave no trace — above all it must not remember a file as processed,
    // which the first live run did, and 86 standings vanished behind a
    // "nothing new" the next morning.
    let today = delivery_date.unwrap_or(prices_max);
    if apply {
        let tx = con.unchecked_transaction()?;
        let (bound, wrote) = feed::write_prices(con, &price_obs, &mut r, &today, "api")?;
        for (eid, says, source) in &evidence_rows {
            evidence(con, eid, says, source, &today)?;
        }
        if let Some(cell) = pen {
            for (id, name, maker) in &pen_rows {
                cell.execute(
                    "INSERT OR IGNORE INTO candidates \
                     (id,kind,name,maker,why,held_since,sellers,low,dimension,body) \
                     VALUES (?1,'model',?2,?3,?4,?5,0,NULL,NULL,'{}')",
                    rusqlite::params![
                        id, name, maker,
                        "a maker the catalogue holds published it; the wire saw it first",
                        today
                    ],
                )?;
            }
        }
        crate::repair::record_run(
            con, "crawler", t.read as i64, bound as i64, wrote as i64, 0.0, "",
        )?;
        // Each board the crawler read this run is cleared whole before its
        // rows go back — the same wholesale replace repair boards uses — so a
        // model that has fallen off a board loses its standing instead of
        // keeping a frozen rank forever. (Per-(entity,suite) replace only
        // touched models still on the board.)
        let mut swept: Vec<&String> = Vec::new();
        for (_, suite, _, _, _, _, _, _) in &standings {
            if !swept.contains(&suite) {
                con.execute("DELETE FROM benchmarks WHERE suite=?1", [suite])?;
                swept.push(suite);
            }
        }
        for (eid, suite, board, value, rank, out_of, metric, source) in &standings {
            con.execute(
                "INSERT OR IGNORE INTO suites (id,name,measurer,url,subject,lower_is_better) \
                 VALUES (?1,?2,'read by the crawler',?3,'model',?4)",
                rusqlite::params![suite, board, source, lower_is_better(metric) as i64],
            )?;
            con.execute(
                "INSERT INTO benchmarks (entity_id,suite,metric,value,rank,out_of,source_url,taken_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![eid, suite, metric, value, rank, out_of, source, today],
            )?;
        }
        for (fname, hash) in &done_files {
            con.execute(
                "INSERT OR REPLACE INTO supply_seen (file, hash, done_at) VALUES (?1,?2,?3)",
                rusqlite::params![fname, hash, today],
            )?;
        }
        tx.commit()?;
    }
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monthly_dollar_figure_reads_as_month_micros() {
        assert_eq!(parse_price("Team plan is $30 / mo"), Some(("month", 30_000_000)));
        assert_eq!(parse_price("$0.25 per 1M tokens"), Some(("mtok_in", 250_000)));
    }

    /// A yearly floor is not turned into a monthly rate — "starting at
    /// $18,000 a year" is a floor, and no arithmetic invents a per-month
    /// price from it.
    #[test]
    fn a_yearly_floor_is_not_made_monthly() {
        assert_eq!(parse_price("Enterprise from $18,000 per year"), None);
    }

    /// A quote the parser cannot reduce is not a price; the caller keeps it
    /// as evidence instead.
    #[test]
    fn an_unpriceable_quote_returns_none() {
        assert_eq!(parse_price("¥12 per thousand tokens"), None);
        assert_eq!(parse_price("Contact sales for pricing"), None);
    }

    #[test]
    fn a_standing_line_reads_rank_field_and_value() {
        let got = parse_standing("1 of 105 on Vectara Hallucination Leaderboard — Hallucination Rate 1.8 %");
        assert_eq!(got, Some((1, 105, "Vectara Hallucination Leaderboard".into(),
                              "Hallucination Rate".into(), 1.8)));
    }
}

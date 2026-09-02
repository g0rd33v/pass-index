//! What the nightly run mends before anyone reads the catalogue.
//!
//! These were Python scripts beside the collectors, one file each. They are
//! moving here a rule at a time, and each is accepted the way `resolve` was:
//! the Python is run against a copy, the Rust against another, and the two
//! catalogues are compared table by table. A step is done when they match.

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The brand is the leading run of letters, not the first word. "QWEN 3.7
/// Plus" and "Qwen3 Max" are one brand written two ways; comparing whole
/// words puts them in separate buckets and mends neither.
fn brand_of(s: &str) -> Option<&str> {
    static B: OnceLock<Regex> = OnceLock::new();
    let re = B.get_or_init(|| Regex::new(r"^([A-Za-z]+)").unwrap());
    re.captures(s).map(|c| c.get(1).unwrap().as_str())
}

/// A seller's id, left alone. Capitalising the first letter of
/// `gpt-oss-safeguard-20b` produces `GPT-oss-safeguard-20b`, an id wearing a
/// hat. Those want naming, which is a different job.
fn is_slug(s: &str) -> bool {
    static A: OnceLock<Regex> = OnceLock::new();
    let re = A.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9._\-]*$").unwrap());
    re.is_match(s)
}

/// One row to mend: the entity, what it said, what it will say, and why.
pub struct Mend {
    pub id: String,
    pub was: String,
    pub now: String,
    pub why: &'static str,
}

/// A brand its own maker writes two ways with neither in the lead.
pub struct Tie {
    pub brand: String,
    pub maker: String,
    pub spellings: Vec<(String, usize)>,
}

pub struct Naming {
    pub brands_split: usize,
    pub mends: Vec<Mend>,
    pub ties: Vec<Tie>,
}

/// One spelling per brand, decided by the company that owns it.
///
/// The same name arrives from a dozen feeds and each shouts or whispers as it
/// pleases: QWEN and Qwen, DeepSeek and Deepseek, NVIDIA and Nvidia. A reader
/// looking at two spellings has no way to tell whether they are two products.
///
/// The authority is the maker's own row, and only for its own models. Black
/// Forest Labs writes FLUX in capitals and Deepgram writes Flux; deciding the
/// spelling across the whole catalogue would rename one of them after the
/// other, which is not a correction but an error with a majority behind it.
///
/// Order is kept everywhere a tie could turn on it — the groups in the order
/// the rows arrive, the spellings in the order they are first seen — because
/// the Python this replaces settles ties that way and the two have to agree.
pub fn naming(con: &Connection) -> Result<Naming> {
    let mut provider: HashMap<String, String> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            provider.insert(r.get(0)?, r.get(1)?);
        }
    }

    // Insertion-ordered groups: a Vec for the order, a map for the lookup.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut members: Vec<Vec<(String, String, String)>> = Vec::new();
    {
        let mut q = con.prepare("SELECT id, name, COALESCE(maker,'') FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let maker: String = r.get(2)?;
            let t = name.trim();
            if is_slug(t) {
                continue;
            }
            let Some(b) = brand_of(t) else { continue };
            let key = (b.to_lowercase(), maker);
            let i = *at.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                members.push(Vec::new());
                members.len() - 1
            });
            members[i].push((id, name.clone(), b.to_string()));
        }
    }

    let mut out = Naming {
        brands_split: 0,
        mends: Vec::new(),
        ties: Vec::new(),
    };

    for (i, (brand, maker)) in order.iter().enumerate() {
        // First-seen order, so equal counts break the way Python's Counter
        // breaks them.
        let mut spellings: Vec<(String, usize)> = Vec::new();
        for (_, _, had) in &members[i] {
            match spellings.iter_mut().find(|(s, _)| s == had) {
                Some((_, n)) => *n += 1,
                None => spellings.push((had.clone(), 1)),
            }
        }
        if spellings.len() < 2 {
            continue;
        }
        out.brands_split += 1;

        let own = provider.get(maker).and_then(|n| brand_of(n.trim()));
        let want: String;
        let why: &'static str;
        match own {
            Some(o)
                if o.to_lowercase() == *brand && spellings.iter().any(|(s, _)| s == o) =>
            {
                want = o.to_string();
                why = "the maker's own name";
            }
            _ => {
                let mut top = spellings.clone();
                // A stable sort on the count alone leaves first-seen order
                // among equals, which is what most_common does.
                top.sort_by(|a, b| b.1.cmp(&a.1));
                if top.len() > 1 && top[0].1 == top[1].1 {
                    out.ties.push(Tie {
                        brand: brand.clone(),
                        maker: provider.get(maker).cloned().unwrap_or_else(|| "—".into()),
                        spellings,
                    });
                    continue;
                }
                want = top[0].0.clone();
                why = "the spelling most of its rows use";
            }
        }

        for (id, name, had) in &members[i] {
            if had != &want {
                out.mends.push(Mend {
                    id: id.clone(),
                    was: name.clone(),
                    now: format!("{}{}", want, &name[had.len()..]),
                    why,
                });
            }
        }
    }
    Ok(out)
}

/// The old spelling stays as an alias, so a feed still using it binds
/// tomorrow rather than minting the name again.
pub fn apply_naming(con: &Connection, r: &Naming) -> Result<()> {
    for m in &r.mends {
        con.execute("UPDATE entities SET name=?1 WHERE id=?2", (&m.now, &m.id))?;
        // `source` is NOT NULL, and OR IGNORE swallows the violation: this
        // insert wrote nothing on every run it ever made, so a renamed brand
        // never kept its old spelling and the feeds re-minted it.
        con.execute(
            "INSERT OR IGNORE INTO aliases (source, alias, entity_id) \
             VALUES ('naming', ?2, ?1)",
            (&m.id, &m.was),
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The run's own record
// ---------------------------------------------------------------------------

/// A job's run, recorded whatever happens to it.
///
/// A source that threw is a source that produced nothing, and that is the
/// fact worth keeping: silence is the failure mode that hides. The row is
/// keyed by source and day, so a second run the same day replaces the first
/// rather than telling the coverage page two stories.
pub fn record_run(
    con: &Connection,
    source: &str,
    read: i64,
    bound: i64,
    written: i64,
    seconds: f64,
    note: &str,
) -> Result<()> {
    // date('now') is UTC, which is what the Python this replaces recorded.
    con.execute(
        "INSERT OR REPLACE INTO source_runs \
         (source,ran_at,fetched,unchanged,read,bound,written,seconds,note) \
         VALUES (?1,date('now'),0,0,?2,?3,?4,?5,?6)",
        rusqlite::params![
            source,
            read,
            bound,
            written,
            (seconds * 10.0).round() / 10.0,
            note
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Corrections the feeds keep re-introducing
// ---------------------------------------------------------------------------

/// The daily checks catch a contradiction; they never fix one, on purpose.
/// But some arrive again every night from a source that will not stop sending
/// them, and reverting those by hand is not a system.
///
/// Nothing here invents a fact. Every rule corrects a field to what the rest
/// of the catalogue already says about the same kind of thing, and says why.
pub struct Rule {
    pub name: &'static str,
    pub find: &'static str,
    pub fix: &'static str,
}

/// A model whose whole purpose is to turn text into a vector does not return
/// text, and 120 of them in the catalogue say `embedding`. The dozen that say
/// `text` are feeds describing the response body rather than the thing
/// produced. Reranking models are left alone: those genuinely return an
/// ordering, not a vector.
pub const RULES: &[Rule] = &[
    Rule {
        name: "an embedding model that says it returns text",
        find: "SELECT name FROM entities \
                WHERE output_kind='text' \
                  AND json_extract(attrs,'$.tasks') LIKE '%\"embedding\"%' \
                  AND json_extract(attrs,'$.tasks') NOT LIKE '%\"rerank\"%'",
        fix: "UPDATE entities SET output_kind='embedding' \
                WHERE output_kind='text' \
                  AND json_extract(attrs,'$.tasks') LIKE '%\"embedding\"%' \
                  AND json_extract(attrs,'$.tasks') NOT LIKE '%\"rerank\"%'",
    },
    // A name that stops inside a bracket is the tail of "(DeepInfra)" with
    // its closing bracket trimmed off upstream. What is inside is a seller or
    // a tier, and neither belongs in the name; cutting it leaves the row
    // under the name the thing actually has, and `fold` then merges it with
    // the row that was already there.
    Rule {
        name: "a name that stops inside a bracket",
        find: "SELECT name FROM entities \
                WHERE name LIKE '%(%' AND name NOT LIKE '%)%'",
        fix: "UPDATE entities SET name = RTRIM(SUBSTR(name, 1, INSTR(name,'(') - 1)) \
                WHERE name LIKE '%(%' AND name NOT LIKE '%)%' \
                  AND LENGTH(RTRIM(SUBSTR(name, 1, INSTR(name,'(') - 1))) > 2",
    },
    // A slash inside a name is a feed writing two names at once, and the
    // address a page is served at cannot carry one.
    Rule {
        name: "two names written as one with a slash",
        find: "SELECT name FROM entities WHERE name LIKE '%/%'",
        fix: "UPDATE entities SET name = REPLACE(name, '/', ' ') \
               WHERE name LIKE '%/%'",
    },
    // A dash or a bracket left hanging at the end is what remains after a
    // serving marker was taken off the tail.
    Rule {
        name: "a name ending in punctuation left behind",
        find: "SELECT name FROM entities \
                WHERE TRIM(name, ' -–—:|/(') <> name",
        fix: "UPDATE entities SET name = TRIM(name, ' -–—:|/(') \
                WHERE TRIM(name, ' -–—:|/(') <> name \
                  AND LENGTH(TRIM(name, ' -–—:|/(')) > 2",
    },
    // A gateway did not build what it resells. Where the maker is an
    // aggregator or a cloud that makes nothing of its own, the catalogue does
    // not know who made it, and saying so is better than crediting the shop.
    Rule {
        name: "a seller standing in for the maker",
        find: "SELECT e.name FROM entities e JOIN providers p ON p.id = e.maker \
                WHERE p.kind IN ('aggregator','cloud') \
                  AND NOT EXISTS(SELECT 1 FROM offerings o JOIN entities x ON x.id = o.entity_id \
                                  WHERE o.provider_id = p.id AND x.maker = p.id AND o.way = 'api')",
        fix: "UPDATE entities SET maker = NULL WHERE id IN ( \
                SELECT e.id FROM entities e JOIN providers p ON p.id = e.maker \
                 WHERE p.kind IN ('aggregator','cloud') \
                   AND NOT EXISTS(SELECT 1 FROM offerings o JOIN entities x ON x.id = o.entity_id \
                                   WHERE o.provider_id = p.id AND x.maker = p.id AND o.way = 'api'))",
    },
    // A model cannot come out before the material it was trained on. The
    // release date is the earliest date a seller gave, and a seller who
    // listed it late gave a date that is not a release at all.
    Rule {
        name: "published before its own training data",
        find: "SELECT name FROM entities \
                WHERE json_extract(attrs,'$.released') < json_extract(attrs,'$.knowledge')",
        fix: "UPDATE entities SET attrs = json_remove(attrs, '$.released') \
                WHERE json_extract(attrs,'$.released') < json_extract(attrs,'$.knowledge')",
    },
];

pub fn normalise(con: &Connection, apply: bool) -> Result<i64> {
    let mut total = 0i64;
    for rule in RULES {
        let mut q = con.prepare(rule.find)?;
        let names: Vec<String> = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        total += names.len() as i64;
        println!("{:<46} {}", rule.name, names.len());
        for n in names.iter().take(8) {
            println!("     {n}");
        }
        if names.len() > 8 {
            println!("     … and {} more", names.len() - 8);
        }
        if apply && !names.is_empty() {
            con.execute(rule.fix, [])?;
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// An alias filed on the wrong row
// ---------------------------------------------------------------------------

/// One alias that names one row and hangs on another.
pub struct Misfiled {
    pub alias: String,
    pub filed_on: String,
    pub names: String,
}

/// An alias is matched before anything is stripped off it, so it is the most
/// exact form there is. Filed on the wrong row it therefore beats the right
/// answer every time, and puts a seller's price on the wrong card.
///
/// Which of the two is right is settled by distance, not by preference.
/// `forms()` returns an alias's shapes ranked by how far they are from what
/// was written, so whichever entity's NAME is reached first is the one the
/// alias reads as. It moves only when that is not the row it sits on, *and*
/// the row it sits on is itself reached by name, later. A row no shape of the
/// alias names is not evidence of anything — a seller's spelling no name
/// matches is exactly what an alias is for. Without that second half the
/// sweep moved 73 aliases and cost 24 bindings their precision.
pub fn misfiled(con: &Connection) -> Result<Vec<Misfiled>> {
    // A form two entities both answer to says nothing about either.
    let mut claims: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            for f in crate::resolve::index_forms(&name) {
                claims.entry(f).or_default().push(id.clone());
            }
        }
    }
    let by_name: HashMap<&str, &str> = claims
        .iter()
        .filter(|(_, ids)| ids.len() == 1)
        .map(|(f, ids)| (f.as_str(), ids[0].as_str()))
        .collect();

    let mut out = Vec::new();
    let mut q = con.prepare("SELECT entity_id, alias FROM aliases")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let eid: String = r.get(0)?;
        let alias: String = r.get(1)?;
        if alias.is_empty() {
            continue;
        }
        let (mut named, mut at, mut mine) = (None, 0usize, None);
        for (i, f) in crate::resolve::forms(&alias).iter().enumerate() {
            if crate::resolve::too_generic(f) {
                continue;
            }
            let Some(&hit) = by_name.get(f.as_str()) else { continue };
            if hit == eid {
                if mine.is_none() {
                    mine = Some(i);
                }
            } else if named.is_none() {
                named = Some(hit.to_string());
                at = i;
            }
            if named.is_some() && mine.is_some() {
                break;
            }
        }
        if let (Some(n), Some(m)) = (named, mine) {
            if at < m {
                out.push(Misfiled {
                    alias,
                    filed_on: eid,
                    names: n,
                });
            }
        }
    }
    Ok(out)
}

/// The nearer name wins; the alias goes with it. One already sitting on the
/// row its own text names is a duplicate rather than a move, so it goes.
pub fn move_aliases(con: &Connection, rows: &[Misfiled]) -> Result<()> {
    for m in rows {
        let held: Option<i64> = con
            .query_row(
                "SELECT 1 FROM aliases WHERE entity_id=?1 AND alias=?2",
                (&m.names, &m.alias),
                |r| r.get(0),
            )
            .ok();
        if held.is_some() {
            con.execute(
                "DELETE FROM aliases WHERE entity_id=?1 AND alias=?2",
                (&m.filed_on, &m.alias),
            )?;
        } else {
            con.execute(
                "UPDATE aliases SET entity_id=?1 WHERE entity_id=?2 AND alias=?3",
                (&m.names, &m.filed_on, &m.alias),
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rows that are the same thing written two ways
// ---------------------------------------------------------------------------

/// Names too common to be evidence of anything. Two companies may both sell
/// a thing called "Web search" and they are not the same product.
const NEVER_FOLD: &[&str] = &[
    "websearch", "webseach", "answer", "contents", "lipsync",
    "replacebackground", "deepresearch", "transcribe", "texttospeech",
    "speechtotext",
];

fn never_fold(f: &str) -> bool {
    crate::resolve::too_generic(f) || NEVER_FOLD.contains(&f)
}

/// A person writes "Qwen3 32B"; a repository writes "Qwen/Qwen3-32B".
fn looks_written(name: &str) -> i32 {
    let mut score = 0;
    if name.contains(' ') {
        score += 2;
    }
    if !name.contains('/') {
        score += 2;
    }
    if !name.contains('-') && !name.contains('_') {
        score += 1;
    }
    if name.chars().next().is_some_and(|c| c.is_uppercase()) {
        score += 1;
    }
    score
}

/// Move an alias whose normalised form equals another row's own name exactly.
///
/// This is what most of the resolver's refusals turned out to be. A feed
/// writes a variant's id, the collector binds it to whichever row it reached
/// first, and from then on two rows answer to one name and the resolver —
/// correctly — refuses to bind either. The rule is exact, not approximate: a
/// near match is left alone, because the whole reason this is repairable is
/// that it is not a guess.
pub fn rehome(con: &Connection, apply: bool) -> Result<usize> {
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            for f in crate::resolve::index_forms(&name) {
                by_name.entry(f).or_default().push(id.clone());
            }
        }
    }
    let owner: HashMap<&str, &str> = by_name
        .iter()
        .filter(|(_, v)| v.len() == 1)
        .map(|(f, v)| (f.as_str(), v[0].as_str()))
        .collect();

    let mut moves: Vec<(String, String, String)> = Vec::new();
    {
        let mut q = con.prepare("SELECT entity_id, alias, source FROM aliases")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let eid: String = r.get(0)?;
            let alias: String = r.get(1)?;
            let source: Option<String> = r.get(2)?;
            if let Some(&should) = owner.get(crate::resolve::norm(&alias).as_str()) {
                if should != eid {
                    moves.push((alias, source.unwrap_or_default(), should.to_string()));
                }
            }
        }
    }
    println!("{} aliases sitting on the wrong row", moves.len());
    if apply {
        for (alias, source, now) in &moves {
            con.execute(
                "UPDATE aliases SET entity_id=?1 WHERE alias=?2 AND source=?3",
                (now, alias, source),
            )?;
        }
    }
    Ok(moves.len())
}

/// One seller, one thing, one rate — written twice.
///
/// Two readings of the same page produce a row with no lane and a row named
/// after the tier, at the identical price. Only a row whose every rate
/// matches another's is folded, so nothing that is actually a different price
/// can be caught by this.
pub fn fold_twin_offerings(con: &Connection, apply: bool) -> Result<usize> {
    let mut q = con.prepare(
        "SELECT a.id, b.id FROM offerings a \
           JOIN offerings b ON b.entity_id=a.entity_id AND b.provider_id=a.provider_id \
                           AND b.way=a.way AND b.id>a.id \
          WHERE COALESCE(a.variant,'')='' \
            AND (SELECT GROUP_CONCAT(dimension||':'||micros_per_unit) \
                   FROM prices WHERE offering_id=a.id) \
              = (SELECT GROUP_CONCAT(dimension||':'||micros_per_unit) \
                   FROM prices WHERE offering_id=b.id)",
    )?;
    let twins: Vec<i64> = q
        .query_map([], |r| r.get::<_, i64>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if apply {
        for drop in &twins {
            con.execute("DELETE FROM prices WHERE offering_id=?1", [drop])?;
            con.execute("DELETE FROM offerings WHERE id=?1", [drop])?;
        }
    }
    Ok(twins.len())
}

/// A fund's name carries suffixes a company's does not — Capital, Ventures,
/// Partners — so folding funds needs its own list. Sequoia and Sequoia
/// Capital are one firm; Pine AI and Pine Labs are two companies, which is
/// why the general rule cannot simply add these words to its own.
fn fund_suffix(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "capital" | "venture" | "ventures" | "partner" | "partners" | "fund"
            | "funds" | "management" | "associates" | "investment" | "investments"
            | "equity" | "collective" | "group" | "lab" | "labs" | "ai"
    )
}

fn bare(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// One fund, one row. Read out of prose, a fund arrives spelled several ways
/// across articles — "Sequoia" in one, "Sequoia Capital" in another — and
/// each spelling made a row with its own slice of the portfolio.
pub fn fold_funds(con: &Connection, apply: bool) -> Result<usize> {
    let mut funds: Vec<(String, String, i64)> = Vec::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers WHERE kind='fund'")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let n: i64 = con.query_row(
                "SELECT COUNT(*) FROM investments WHERE fund_id=?1",
                [&id],
                |r| r.get(0),
            )?;
            funds.push((id, name, n));
        }
    }
    let mut used: Vec<String> = Vec::new();
    let mut merged = 0usize;
    for i in 0..funds.len() {
        if used.contains(&funds[i].0) {
            continue;
        }
        let mut group = vec![funds[i].clone()];
        used.push(funds[i].0.clone());
        for j in 0..funds.len() {
            if used.contains(&funds[j].0) {
                continue;
            }
            let (x, y) = (bare(&funds[i].1), bare(&funds[j].1));
            let (lo, hi) = if x.len() <= y.len() { (&x, &y) } else { (&y, &x) };
            if !lo.is_empty() && hi.starts_with(lo.as_str()) && fund_suffix(&hi[lo.len()..]) {
                group.push(funds[j].clone());
                used.push(funds[j].0.clone());
            }
        }
        if group.len() < 2 {
            continue;
        }
        // The fuller name, and the one that has actually backed more.
        group.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.len().cmp(&a.1.len())));
        let keep = group[0].0.clone();
        if apply {
            for d in &group[1..] {
                con.execute(
                    "UPDATE OR IGNORE investments SET fund_id=?1 WHERE fund_id=?2",
                    (&keep, &d.0),
                )?;
                con.execute(
                    "DELETE FROM investments WHERE fund_id=?1 OR company_id=?1",
                    [&d.0],
                )?;
                con.execute("DELETE FROM providers WHERE id=?1", [&d.0])?;
            }
        }
        merged += group.len() - 1;
    }
    Ok(merged)
}

/// Rows that are the same thing written two ways, found by the resolver's own
/// normalisation rather than by comparing names.
///
/// Which row survives is decided by what is attached to it: the one with more
/// prices and standings, and on a tie the one whose name a person would
/// write. Everything hanging off the loser moves across before it goes, and
/// its name is kept as an alias so a feed still using it binds tomorrow.
pub fn fold_entities(con: &Connection, apply: bool) -> Result<usize> {
    let mut info: Vec<(String, String, String, String)> = Vec::new();
    {
        let mut q =
            con.prepare("SELECT id, name, register, COALESCE(maker,'') FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            info.push((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?));
        }
    }
    let mut weight: HashMap<String, i64> = info.iter().map(|e| (e.0.clone(), 0)).collect();
    for (sql, _) in [
        ("SELECT o.entity_id, COUNT(*) FROM offerings o JOIN prices p ON p.offering_id=o.id GROUP BY 1", 0),
        ("SELECT entity_id, COUNT(*) FROM benchmarks GROUP BY 1", 0),
    ] {
        let mut q = con.prepare(sql)?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let n: i64 = r.get(1)?;
            *weight.entry(id).or_insert(0) += n;
        }
    }

    // A version that changed places is the reordering the incoming side does
    // and this did not, so "Claude Opus 4.8" and "Claude 4.8 Opus" never met
    // here and the catalogue held both.
    let mut claims: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (_, name, _, _)) in info.iter().enumerate() {
        let mut forms = crate::resolve::index_forms(name);
        forms.extend(crate::resolve::index_forms(&crate::resolve::swap_version(name)));
        for f in forms {
            if !never_fold(&f) {
                claims.entry(f).or_default().push(i);
            }
        }
    }

    // Only rows of the same register and the same maker: a tool and a model
    // that share a name are not one thing.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut seen: Vec<Vec<usize>> = Vec::new();
    let mut keys: Vec<&String> = claims.keys().collect();
    keys.sort();
    for f in keys {
        let ids = &claims[f];
        if ids.len() < 2 {
            continue;
        }
        let mut by_key: HashMap<(String, String), Vec<usize>> = HashMap::new();
        for &i in ids {
            by_key
                .entry((info[i].2.clone(), info[i].3.clone()))
                .or_default()
                .push(i);
        }
        for (_, mut same) in by_key {
            if same.len() > 1 {
                same.sort();
                same.dedup();
                if same.len() > 1 && !seen.contains(&same) {
                    seen.push(same.clone());
                    groups.push(same);
                }
            }
        }
    }

    println!("{} groups to fold", groups.len());
    let mut folded = 0usize;
    for g in &groups {
        let mut ranked = g.clone();
        ranked.sort_by(|&a, &b| {
            let wa = *weight.get(&info[a].0).unwrap_or(&0);
            let wb = *weight.get(&info[b].0).unwrap_or(&0);
            wb.cmp(&wa)
                .then(looks_written(&info[b].1).cmp(&looks_written(&info[a].1)))
        });
        let keep = &info[ranked[0]].0;
        println!(
            "  keep {:<34} ({} attached)",
            info[ranked[0]].1.chars().take(34).collect::<String>(),
            weight.get(keep).unwrap_or(&0)
        );
        for &d in &ranked[1..] {
            let drop = &info[d].0;
            println!(
                "     fold in {:<34} ({} attached)",
                info[d].1.chars().take(34).collect::<String>(),
                weight.get(drop).unwrap_or(&0)
            );
            folded += 1;
            if !apply {
                continue;
            }
            con.execute(
                "INSERT OR IGNORE INTO aliases (source, alias, entity_id) VALUES ('fold',?1,?2)",
                (&info[d].1, keep),
            )?;
            for sql in [
                "UPDATE OR IGNORE aliases SET entity_id=?1 WHERE entity_id=?2",
                "UPDATE OR IGNORE offerings SET entity_id=?1 WHERE entity_id=?2",
                "UPDATE OR IGNORE benchmarks SET entity_id=?1 WHERE entity_id=?2",
                "UPDATE OR IGNORE docs SET subject=?1 WHERE subject=?2",
                "UPDATE entities SET derived_from=?1 WHERE derived_from=?2",
            ] {
                con.execute(sql, (keep, drop))?;
            }
            // An offering that could not move — because the survivor already
            // has one from that seller on that lane — is deleted, and its
            // prices have to go with it. Deleting the offering alone left
            // twenty price rows pointing at nothing.
            con.execute("DELETE FROM aliases WHERE entity_id=?1", [drop])?;
            con.execute(
                "DELETE FROM prices WHERE offering_id IN \
                 (SELECT id FROM offerings WHERE entity_id=?1)",
                [drop],
            )?;
            for sql in [
                "DELETE FROM offerings WHERE entity_id=?1",
                "DELETE FROM benchmarks WHERE entity_id=?1",
                "DELETE FROM docs WHERE subject=?1",
                "DELETE FROM entities WHERE id=?1",
            ] {
                con.execute(sql, [drop])?;
            }
        }
    }
    Ok(folded)
}

// ---------------------------------------------------------------------------
// How big a model is, where its own name says so
// ---------------------------------------------------------------------------

/// A model's size decides where it can run, and the parameter count is the
/// only figure that answers it. The catalogue held that count for a tenth of
/// its models, because only a model card states it in a field — but the
/// makers put it in the name of hundreds more.
///
/// Two readings and one refusal. A mixture of experts carries two numbers,
/// the total and the active, and the total is the one that decides whether it
/// fits in your memory, so "397B-A17B" reads as 397. A closed model whose
/// maker publishes no count is left unsized: guessing it from a price would
/// invent the one number the page exists to report.
struct SizePats {
    /// "70B", "1.8B", "480b" — a number with a B on it, not part of a longer
    /// word. Rust has no look-around, so the guards are matched as characters
    /// and stepped over.
    size: Regex,
    /// "235B-A22B", "397B A17B": total then active.
    moe: Regex,
}

fn size_pats() -> &'static SizePats {
    static P: OnceLock<SizePats> = OnceLock::new();
    P.get_or_init(|| SizePats {
        size: Regex::new(r"(?:^|[^a-zA-Z0-9.])(\d+(?:\.\d+)?)\s*[bB](?:$|[^a-zA-Z0-9])").unwrap(),
        moe: Regex::new(r"(\d+(?:\.\d+)?)\s*[bB][\s\-]*[aA](\d+(?:\.\d+)?)\s*[bB]").unwrap(),
    })
}

/// Every match of a pattern whose guards are consumed characters, restarting
/// one character back so two counts separated by a single space are both
/// seen — "17B 16E" would otherwise lose the second to the first's guard.
fn all_sizes(name: &str) -> Vec<f64> {
    let re = &size_pats().size;
    let b = name.as_bytes();
    let mut out = Vec::new();
    let mut from = 0usize;
    while from <= b.len() {
        let Some(c) = re.captures_at(name, from) else { break };
        let whole = c.get(0).unwrap();
        if let Ok(v) = c.get(1).unwrap().as_str().parse::<f64>() {
            out.push(v);
        }
        // Step back over the trailing guard, which may begin the next match.
        let next = whole.end().saturating_sub(1).max(whole.start() + 1);
        if next <= from {
            break;
        }
        from = next;
    }
    out
}

/// The parameter count the name states, in billions, and how it read.
pub fn read_size(name: &str) -> Option<(f64, &'static str)> {
    if let Some(c) = size_pats().moe.captures(name) {
        if let Ok(v) = c.get(1).unwrap().as_str().parse::<f64>() {
            return Some((v, "its own name, the total of a mixture"));
        }
    }
    // A name can carry two numbers — "Llama 4 Scout 17B 16E" — and the
    // parameter count is the larger; the other counts experts or context.
    let hits = all_sizes(name);
    if hits.is_empty() {
        return None;
    }
    let max = hits.into_iter().fold(f64::MIN, f64::max);
    Some((max, "its own name"))
}

pub fn sizes(con: &Connection, apply: bool) -> Result<usize> {
    let mut writes: Vec<(String, i64, &'static str)> = Vec::new();
    let mut already = 0usize;
    {
        let mut q = con.prepare(
            "SELECT id, name, COALESCE(attrs,'{}') FROM entities WHERE register='model'",
        )?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let attrs: String = r.get(2)?;
            let at: serde_json::Value =
                serde_json::from_str(&attrs).unwrap_or(serde_json::Value::Null);
            // Python's truth test: absent, null, 0 and "" all mean unstated.
            let stated = match at.get("params") {
                None | Some(serde_json::Value::Null) => false,
                Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
                Some(serde_json::Value::String(s)) => !s.is_empty(),
                Some(serde_json::Value::Bool(b)) => *b,
                Some(v) => !v.as_array().is_some_and(|a| a.is_empty()),
            };
            if stated {
                already += 1;
                continue;
            }
            let Some((billions, why)) = read_size(&name) else { continue };
            if billions <= 0.0 || billions > 100_000.0 {
                continue;
            }
            writes.push((id, (billions * 1e9).round() as i64, why));
        }
    }
    println!("models the catalogue can size: {}", writes.len() + already);
    if apply {
        for (id, params, why) in &writes {
            con.execute(
                "UPDATE entities SET attrs = json_set(coalesce(attrs,'{}'), '$.params', ?1, \
                 '$.params_read_from', ?2) WHERE id = ?3",
                rusqlite::params![params, why, id],
            )?;
        }
        println!("\nwrote {}", writes.len());
    } else {
        println!("\ndry run; nothing written");
    }
    Ok(writes.len())
}

// ---------------------------------------------------------------------------
// Companies kept although nobody can price them, and why
// ---------------------------------------------------------------------------

/// The significance of a company that publishes no price cannot be computed
/// from a catalogue of prices — the very fact that would make it computable
/// is the one that is missing. So it is written down, with a reason, and the
/// reason is a judgement somebody made and signed rather than a number
/// pretending not to be one.
///
/// A company here is one we assert belongs in the catalogue. A company in the
/// waiting room with neither an entry here nor anything pointing at it is a
/// name nobody has justified, and the page says so, because a catalogue that
/// never removes anything is a list.
pub const KEPT: &[(&str, &str)] = &[
    ("Harvey", "legal work at the largest firms; priced by contract, nothing published"),
    ("Sierra", "customer-service agents, sold per resolved conversation; no public rate card"),
    ("Decagon", "customer-service agents sold to enterprises; no public rate card"),
    ("Glean", "enterprise search and agents; the pricing page is a demo form"),
    ("Cresta", "contact-centre agents, sold by contract"),
    ("Parloa", "voice agents for contact centres, sold by contract"),
    ("Hebbia", "document analysis for finance and law; priced by contract"),
    ("EvenUp", "personal-injury case work; priced by contract"),
    ("Abridge", "clinical documentation, sold to health systems"),
    ("Nabla", "clinical documentation, sold to health systems"),
    ("Ambience Healthcare", "clinical documentation, sold to health systems"),
    ("Windsurf", "folded into Devin; windsurf.com now redirects to devin.ai/pricing"),
    ("Factory", "software agents sold to engineering organisations"),
    ("Augment Code", "software agents sold to engineering organisations"),
    ("Cognition", "makes Devin, which is priced here by the month"),
    ("Suno", "priced here by the month"),
    ("Udio", "music generation; the pricing page states no figure a reader can quote"),
    ("Leonardo.Ai", "image generation; the pricing page refuses a plain request"),
    ("Freepik", "image and video generation bundled into a design subscription"),
    ("Lambda", "GPU hire rather than a model; priced per instance-hour"),
    ("Hyperbolic", "GPU hire and inference; the price list needs an account"),
    ("PlayAI", "voice, sold by contract"),
    ("Synthflow", "voice agents for contact centres, sold by contract"),
    ("Neuphonic", "voice, sold by contract"),
    ("Bright Data", "web data collection; priced per record, not per token"),
    ("Docsumo", "document extraction, sold by contract"),
    ("Rossum", "document extraction, sold by contract"),
    ("Mirage (Captions)", "video generation inside a consumer app"),
    ("Sloyd", "3D asset generation, sold by seat"),
    ("Scenario", "game-asset generation, sold by seat"),
    ("Loudly", "music generation, sold by subscription"),
    ("CSM", "3D world generation; no public rate card"),
    ("Klavis AI", "hosted connectors; no published price"),
    ("Smithery", "a registry of connectors rather than something sold"),];

pub struct Opaque {
    pub known: Vec<(String, String, &'static str)>,
    pub absent: Vec<&'static str>,
    pub unjustified: Vec<String>,
}

pub fn opaque(con: &Connection) -> Result<Opaque> {
    let mut have: HashMap<String, String> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            have.insert(name, id);
        }
    }
    let mut out = Opaque {
        known: Vec::new(),
        absent: Vec::new(),
        unjustified: Vec::new(),
    };
    for (name, why) in KEPT {
        match have.get(*name) {
            Some(id) => out.known.push((id.clone(), (*name).to_string(), *why)),
            None => out.absent.push(name),
        }
    }
    // Everyone in the waiting room with no reason here and nothing pointing
    // at them: nobody has said why the catalogue holds the name.
    let mut q = con.prepare(
        "SELECT p.name FROM providers p \
          WHERE NOT EXISTS (SELECT 1 FROM offerings o WHERE o.provider_id = p.id) \
            AND NOT EXISTS (SELECT 1 FROM entities e WHERE e.maker = p.id) \
            AND NOT EXISTS (SELECT 1 FROM docs d WHERE d.subject = p.id) \
            AND COALESCE(p.notes,'') = '' \
          ORDER BY p.name",
    )?;
    out.unjustified = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(out)
}

pub fn apply_opaque(con: &Connection, o: &Opaque) -> Result<()> {
    for (pid, _name, why) in &o.known {
        con.execute("UPDATE providers SET notes=?1 WHERE id=?2", (why, pid))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// What a thing is FOR
// ---------------------------------------------------------------------------

/// Modality cannot separate the hundreds of things that all put out text: a
/// chat model, a reasoner, a coder, a reranker, an OCR and a guard model are
/// one column to the catalogue and six different products to a buyer.
///
/// The tag comes from evidence the catalogue already holds — the modality,
/// the maker's own name for the thing, its own description, and the boards it
/// is measured on — and never from a guess about what a model is probably
/// good at.
const REASONING_BOARDS: &[&str] = &[
    "aime_2025", "aime_2026", "gpqa_diamond", "epoch_frontiermath", "epoch_hle",
    "hle", "arc_agi_1", "arc_agi_2", "arc_agi_3", "epoch_otis_mock_aime",
    "mmlu_pro",
];
const CODE_BOARDS: &[&str] = &[
    "swebench_verified", "swebench_verified_all_agents", "swebench_pro_public",
    "swebench_multimodal", "livecodebench", "aider_polyglot", "swe_rebench",
    "terminal_bench_2_0", "terminal_bench_2_1", "lmarena_webdev",
];
// The agent boards are exactly the Top page's Agents niche, so they are read
// from it rather than copied — a copy drifted the moment a board was added to
// one and not the other. REASONING_BOARDS and CODE_BOARDS above stay their
// own lists on purpose: they are deliberately narrower than the niche's full
// board set, a subset chosen for tagging, not the same list twice.
fn agent_boards() -> &'static [&'static str] {
    crate::top::niche("agents").map(|n| n.boards).unwrap_or(&[])
}

fn tags_for(
    name: &str,
    desc: &str,
    ink: &str,
    outk: &str,
    register: &str,
    suites: &[String],
) -> Vec<&'static str> {
    let n = name.to_lowercase();
    let both = format!("{n} {desc}");
    let has = |ws: &[&str]| ws.iter().any(|w| both.contains(w));
    let named = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    let on = |board: &[&str]| suites.iter().any(|s| board.contains(&s.as_str()));
    let mut t: Vec<&'static str> = Vec::new();

    if outk == "embedding" || named(&["embedding", "embed"]) {
        t.push(if has(&["rerank"]) { "rerank" } else { "embedding" });
    }
    if named(&["rerank"]) && !t.contains(&"rerank") {
        t.push("rerank");
    }
    // Audio in and text out describes a transcriber AND every omni chat
    // model. Only the one that says so, or that takes nothing but audio, is
    // a transcriber.
    if ink.contains("audio")
        && outk.contains("text")
        && (ink == "audio"
            || has(&[
                "transcri", "speech-to-text", "speech to text", "asr", "diariz",
                "subtitl",
            ]))
    {
        t.push("transcribe");
    }
    // A video model whose clips carry sound is not a speech model.
    if outk.contains("audio") && !outk.contains("video") {
        t.push(if has(&["music", "song", "soundtrack", "compose"]) {
            "music"
        } else {
            "speak"
        });
    }
    if outk.contains("image") {
        t.push("image");
    }
    if outk.contains("video") {
        t.push(
            if has(&["avatar", "lip-sync", "lipsync", "talking head"]) {
                "avatar"
            } else {
                "video"
            },
        );
    }
    if outk.contains("text") || outk.contains("code") {
        if has(&["ocr", "document understanding", "reads documents"]) {
            t.push("ocr");
            // It reads pictures, it does not draw them.
            t.retain(|x| *x != "image");
        }
        if outk.contains("code")
            || named(&["code", "coder", "codex", "devstral", "starcoder"])
        {
            t.push("code");
        }
        if has(&["translat"]) {
            t.push("translate");
        }
        if has(&[
            "guardrail", "moderation", "safety classifier", "content safety",
            "shieldgemma", "llama guard",
        ]) {
            t.push("guard");
        }
        if has(&[
            "web search", "search the web", "grounding", "deep research",
            "retrieval-augmented",
        ]) {
            t.push("search");
        }
        // The web and data tooling the vocabulary was missing entirely.
        if has(&[
            "crawl", "scrape", "render the page", "page render",
            "browser session", "sitemap",
        ]) {
            t.push("crawl");
        }
        if has(&[
            "extract", "parse", "structured output from", "spellcheck",
            "autosuggest", "answers",
        ]) {
            t.push("extract");
        }
        if has(&["sandbox", "code execution", "run code", "code interpreter"]) {
            t.push("sandbox");
        }
        if has(&["evaluat", "trace", "observability", "llm-as-a-judge"]) {
            t.push("evaluate");
        }
        if named(&["thinking", "reasoning", "reasoner"]) || on(REASONING_BOARDS) {
            t.push("reasoning");
        }
        if on(CODE_BOARDS) && !t.contains(&"code") {
            t.push("code");
        }
        if on(agent_boards()) {
            t.push("agents");
        }
    }
    // Anything that answers in prose and claiming nothing more specific is a
    // chat model; that is a real category, not a leftover bin.
    if t.is_empty()
        && (outk.contains("text") || outk.contains("code"))
        && (register == "model" || register == "agent")
    {
        t.push("chat");
    }
    let mut seen: Vec<&'static str> = Vec::new();
    for x in t {
        if !seen.contains(&x) {
            seen.push(x);
        }
    }
    seen
}

pub fn tasks(con: &Connection, apply: bool) -> Result<(usize, usize)> {
    let mut desc: HashMap<String, String> = HashMap::new();
    {
        let mut q =
            con.prepare("SELECT subject, text FROM docs WHERE kind='description'")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let s: String = r.get(0)?;
            let t: String = r.get(1)?;
            desc.entry(s).or_insert_with(|| t.to_lowercase());
        }
    }
    let mut suites: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut q = con.prepare("SELECT DISTINCT entity_id, suite FROM benchmarks")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            suites.entry(r.get(0)?).or_default().push(r.get(1)?);
        }
    }
    let mut all: Vec<(String, Vec<&'static str>)> = Vec::new();
    let mut untagged = 0usize;
    {
        let mut q = con
            .prepare("SELECT id, name, input_kind, output_kind, register FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let ink: Option<String> = r.get(2)?;
            let outk: Option<String> = r.get(3)?;
            let register: String = r.get(4)?;
            let empty = Vec::new();
            let t = tags_for(
                &name,
                desc.get(&id).map(String::as_str).unwrap_or(""),
                ink.as_deref().unwrap_or(""),
                outk.as_deref().unwrap_or(""),
                &register,
                suites.get(&id).unwrap_or(&empty),
            );
            if t.is_empty() {
                untagged += 1;
                continue;
            }
            all.push((id, t));
        }
    }
    println!(
        "{} of {} tagged, {} left untagged",
        all.len(),
        all.len() + untagged,
        untagged
    );
    if apply {
        for (id, t) in &all {
            let json = serde_json::to_string(t)?;
            con.execute(
                "UPDATE entities SET attrs = json_set(COALESCE(NULLIF(attrs,''),'{}'), \
                 '$.tasks', json(?1)) WHERE id = ?2",
                (&json, id),
            )?;
        }
    }
    Ok((all.len(), untagged))
}

// ---------------------------------------------------------------------------
// An offering nobody has confirmed lately
// ---------------------------------------------------------------------------

/// A withdrawn listing used to keep its price forever: nothing ever moved an
/// offering off 'live', so a seller who dropped a model still owned its
/// headline rate. An offering goes stale when the collectors have not seen
/// it for a week WHILE they are demonstrably still reading that seller —
/// silence about one row during a working read is the seller saying it is
/// gone; silence because the whole source is broken says nothing, and marks
/// nothing. The witness that the seller was read must sell the same way:
/// Anthropic's subscriptions job runs nightly while its rate card sits
/// unchanged for a week, and a fresh subscription row was about to shelve
/// Claude Opus 5. Sighting revives a row (the upsert already does).
/// A leaderboard standing older than 45 days is a board that stopped being
/// read — its scraper's HTML drifted and now parses to nothing, or the board
/// went away — while the card kept serving last month's rank as current. A
/// board that IS read rewrites every row's taken_at to today each night
/// (write_standings and the crawler both DELETE+INSERT), so only a genuinely
/// frozen board has old rows; 45 days is far past any real refresh gap.
pub fn expire_standings(con: &Connection, today: &str, apply: bool) -> Result<usize> {
    let mut q = con.prepare(
        "SELECT b.suite, COUNT(*) FROM benchmarks b \
          WHERE b.taken_at < date(?1, '-45 day') GROUP BY b.suite ORDER BY 2 DESC",
    )?;
    let stale: Vec<(String, i64)> = q
        .query_map([today], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);
    let total: i64 = stale.iter().map(|(_, n)| n).sum();
    println!("standings frozen past 45 days: {total}");
    for (suite, n) in stale.iter().take(12) {
        println!("     {suite}  ({n})");
    }
    if apply && total > 0 {
        con.execute("DELETE FROM benchmarks WHERE taken_at < date(?1, '-45 day')", [today])?;
    }
    Ok(total as usize)
}

pub fn retire(con: &Connection, today: &str, apply: bool) -> Result<usize> {
    let sql = "SELECT o.id, e.name || ' at ' || p.name
                 FROM offerings o
                 JOIN entities e ON e.id = o.entity_id
                 JOIN providers p ON p.id = o.provider_id
                WHERE o.status = 'live'
                  AND o.way IN ('api','aggregator','cloud')
                  AND o.last_seen < date(?1, '-7 day')
                  AND EXISTS (SELECT 1 FROM offerings o2
                               WHERE o2.provider_id = o.provider_id
                                 AND o2.way = o.way
                                 AND o2.last_seen >= date(?1, '-2 day'))";
    let mut q = con.prepare(sql)?;
    let rows: Vec<(i64, String)> = q
        .query_map([today], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);
    println!("offerings the seller has stopped listing: {}", rows.len());
    for (_, name) in rows.iter().take(12) {
        println!("     {name}");
    }
    if rows.len() > 12 {
        println!("     … and {} more", rows.len() - 12);
    }
    if apply {
        for (id, _) in &rows {
            con.execute("UPDATE offerings SET status='stale' WHERE id=?1", [id])?;
        }
    }
    Ok(rows.len())
}

// ---------------------------------------------------------------------------
// The holding pen
// ---------------------------------------------------------------------------

/// A second database for what has not been verified.
///
/// The main catalogue is what somebody stands behind. Missing an entry from
/// it costs a reader one lookup; a wrong entry costs them trust in every other
/// row, so the two are not comparable and the pen is a separate file rather
/// than a flag: the product opens `index.db` and cannot reach what is not in
/// it, whatever query anybody writes later.
pub const PEN_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS candidates (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    maker      TEXT,
    why        TEXT NOT NULL,
    held_since TEXT NOT NULL,
    sellers    INTEGER NOT NULL DEFAULT 0,
    low        INTEGER,
    dimension  TEXT,
    body       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS cand_kind ON candidates(kind);
CREATE INDEX IF NOT EXISTS cand_why ON candidates(why);
";

/// A candidate carries everything it arrived with — its sellers, its prices,
/// why it is here — so that promoting it is a copy and not a re-crawl, and so
/// that somebody reviewing it can see what the feeds actually said.
fn body_of(main: &Connection, eid: &str, unvetted: &HashMap<String, String>)
    -> Result<(String, usize, Option<i64>, Option<String>)>
{
    let (ink, outk, attrs): (Option<String>, Option<String>, String) = main.query_row(
        "SELECT input_kind, output_kind, COALESCE(attrs,'{}') FROM entities WHERE id=?1",
        [eid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let mut offers: Vec<serde_json::Value> = Vec::new();
    let (mut low, mut dim): (Option<i64>, Option<String>) = (None, None);
    let rows: Vec<(i64, String, String, String)> = {
        let mut q = main.prepare(
            "SELECT id, provider_id, way, COALESCE(variant,'') FROM offerings WHERE entity_id=?1",
        )?;
        let v: Vec<(i64, String, String, String)> = q
            .query_map([eid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    for (oid, pid, way, variant) in rows {
        let mut px: Vec<serde_json::Value> = Vec::new();
        let mut q = main.prepare(
            "SELECT dimension, micros_per_unit, source_url FROM prices WHERE offering_id=?1",
        )?;
        let mut pr = q.query([oid])?;
        while let Some(r) = pr.next()? {
            let d: String = r.get(0)?;
            let m: i64 = r.get(1)?;
            let s: String = r.get(2)?;
            if matches!(d.as_str(), "mtok_in" | "image" | "call" | "minute" | "second")
                && low.is_none_or(|l| m < l)
            {
                low = Some(m);
                dim = Some(d.clone());
            }
            px.push(serde_json::json!({"dimension": d, "micros": m, "source": s}));
        }
        offers.push(serde_json::json!({
            "seller": unvetted.get(&pid).cloned().unwrap_or(pid),
            "way": way, "variant": variant, "prices": px
        }));
    }
    let aliases: Vec<String> = {
        let mut q = main.prepare("SELECT alias FROM aliases WHERE entity_id=?1")?;
        let v: Vec<String> = q
            .query_map([eid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };
    let body = serde_json::json!({
        "input_kind": ink, "output_kind": outk,
        "attrs": serde_json::from_str::<serde_json::Value>(&attrs)
            .unwrap_or(serde_json::json!({})),
        "offerings": offers, "aliases": aliases
    });
    let n = body["offerings"].as_array().map(|a| a.len()).unwrap_or(0);
    Ok((body.to_string(), n, low, dim))
}

/// Move out what only an unvetted company sells, and those companies.
///
/// A company we chose has a home page, because somebody wrote one down. A
/// company that exists only because a feed used its key as a provider name
/// has not been looked at, and a model whose only seller is one of those is a
/// price nobody here has stood behind.
pub fn hold_unvetted(
    main: &Connection,
    cell: &Connection,
    today: &str,
    apply: bool,
) -> Result<(usize, usize)> {
    let mut unvetted: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    {
        let mut q =
            main.prepare("SELECT id, name FROM providers WHERE COALESCE(url,'') = ''")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            order.push(id.clone());
            unvetted.insert(id, name);
        }
    }
    if unvetted.is_empty() {
        return Ok((0, 0));
    }
    let marks = vec!["?"; unvetted.len()].join(",");
    // A thing whose only seller is unvetted still goes; a company does not,
    // if anybody funded it.
    let only: Vec<String> = {
        let sql = format!(
            "SELECT e.id FROM entities e \
              WHERE EXISTS(SELECT 1 FROM offerings o WHERE o.entity_id=e.id \
                             AND o.provider_id IN ({marks})) \
                AND NOT EXISTS(SELECT 1 FROM offerings o JOIN providers p ON p.id=o.provider_id \
                                WHERE o.entity_id=e.id AND COALESCE(p.url,'') <> '')"
        );
        let mut q = main.prepare(&sql)?;
        let v: Vec<String> = q
            .query_map(rusqlite::params_from_iter(order.iter()), |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        v
    };

    let mut moved = 0usize;
    for eid in &only {
        let row: Option<(String, String, String)> = main
            .query_row(
                "SELECT register, name, COALESCE(maker,'') FROM entities WHERE id=?1",
                [eid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        let Some((register, name, maker)) = row else { continue };
        let maker_name: Option<String> = main
            .query_row("SELECT name FROM providers WHERE id=?1", [&maker], |r| r.get(0))
            .ok();
        let (body, sellers, low, dim) = body_of(main, eid, &unvetted)?;
        moved += 1;
        // The pen write belongs with the deletes and not beside them. It used
        // to run whether or not this was a real pass, so every dry run put a
        // row in the pen and left it in the catalogue too — 326 of them, and
        // a row in both databases is the one thing the pen exists to prevent.
        if apply {
            cell.execute(
                "INSERT OR REPLACE INTO candidates \
                 (id,kind,name,maker,why,held_since,sellers,low,dimension,body) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                rusqlite::params![
                    eid, register, name, maker_name,
                    "no company we have checked sells it", today,
                    sellers as i64, low, dim, body
                ],
            )?;
            main.execute(
                "DELETE FROM prices WHERE offering_id IN \
                 (SELECT id FROM offerings WHERE entity_id=?1)",
                [eid],
            )?;
            for sql in [
                "DELETE FROM offerings WHERE entity_id=?1",
                "DELETE FROM benchmarks WHERE entity_id=?1",
                "DELETE FROM aliases WHERE entity_id=?1",
                // Its docs go too — a swept model's description or evidence
                // rows would otherwise point at an id no longer in the
                // catalogue, which the orphans check then flags every night.
                "DELETE FROM docs WHERE subject=?1",
                "DELETE FROM entities WHERE id=?1",
            ] {
                main.execute(sql, [eid])?;
            }
        }
    }

    let swept: std::collections::HashSet<&String> = only.iter().collect();
    let mut companies = 0usize;
    for pid in &order {
        // What this company would still be selling once the sweep is done.
        // Counting the rows still present instead made the dry run and the
        // real run disagree: after the deletes above there is nothing left,
        // before them there is everything.
        let left = {
            let mut q = main.prepare("SELECT entity_id FROM offerings WHERE provider_id=?1")?;
            let sold: Vec<String> =
                q.query_map([pid], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            sold.iter().filter(|e| !swept.contains(e)).count() as i64
        };
        // A maker is not a seller. Sweeping out companies nobody vetted as a
        // seller took with it the companies whose only role here is having
        // built something we hold, and left 79 models pointing at a row that
        // no longer existed.
        let makes: i64 =
            main.query_row("SELECT COUNT(*) FROM entities WHERE maker=?1", [pid], |r| r.get(0))?;
        if makes > 0 {
            continue;
        }
        // A company a fund names in its own portfolio has been looked at by
        // somebody who put money in it. The sweep took 35 of one collective's
        // companies the same night they arrived, for the crime of having a
        // website nobody could find.
        let invested: i64 = main.query_row(
            "SELECT COUNT(*) FROM investments WHERE company_id=?1 OR fund_id=?1",
            [pid],
            |r| r.get(0),
        )?;
        if invested > 0 {
            continue;
        }
        if left > 0 {
            continue; // it still sells something that stayed
        }
        companies += 1;
        if apply {
            cell.execute(
                "INSERT OR REPLACE INTO candidates \
                 (id,kind,name,maker,why,held_since,sellers,low,dimension,body) \
                 VALUES (?1,'company',?2,NULL,?3,?4,0,NULL,NULL,'{}')",
                rusqlite::params![pid, unvetted[pid], "a company we have not looked at", today],
            )?;
            main.execute("DELETE FROM providers WHERE id=?1", [pid])?;
        }
    }
    Ok((moved, companies))
}

/// A company that has since arrived in the catalogue has been let in by a
/// road somebody stands behind — a fund's own portfolio, Y Combinator's
/// directory — and holding it in the pen too would put one name in both
/// databases. Companies only: an entity that "arrives" arrives because a
/// feed re-minted it, and forgetting the refusal is how newmodels and
/// quarantine spent five nights re-processing the same three hundred rows.
pub fn release_arrived(main: &Connection, cell: &Connection) -> Result<usize> {
    let ids: Vec<String> = {
        let mut q = cell.prepare("SELECT id FROM candidates WHERE kind='company'")?;
        let v: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    let mut released = 0usize;
    for id in ids {
        let here: bool = main
            .query_row("SELECT 1 FROM providers WHERE id=?1", [&id], |r| r.get::<_, i64>(0))
            .is_ok();
        if here {
            cell.execute("DELETE FROM candidates WHERE id=?1", [&id])?;
            released += 1;
        }
    }
    Ok(released)
}

// ---------------------------------------------------------------------------
// One description per thing
// ---------------------------------------------------------------------------

/// A card prints one description, and it held up to five.
///
/// Which one a reader saw was decided by the order rows came back in, not by
/// a rule — the same fault the resolver had, in the text rather than in the
/// binding. The rule is the one the catalogue already states for two sources
/// disagreeing about a rate: **the maker's own page wins; among third
/// parties, the most recent reading.** Ties are broken by the address, so the
/// answer does not move between runs.
///
/// The losers are deleted rather than kept and hidden. A reseller's blurb is
/// not a figure somebody published about what a thing costs; it is prose, and
/// the catalogue's promise is about the figures. Keeping four unread copies
/// of it buys nothing and leaves the card's choice looking like a coin toss.
pub fn one_description(con: &Connection, apply: bool) -> Result<usize> {
    // Every subject with more than one, and the maker's own host if it has one.
    let mut subjects: Vec<String> = Vec::new();
    {
        let mut q = con.prepare(
            "SELECT subject FROM docs WHERE kind='description' \
              GROUP BY subject HAVING COUNT(*) > 1 ORDER BY subject",
        )?;
        let v: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        subjects = v;
    }
    let mut dropped = 0usize;
    for s in &subjects {
        // The maker's own address, where the subject is a thing somebody made.
        let host: Option<String> = con
            .query_row(
                "SELECT p.url FROM entities e JOIN providers p ON p.id = e.maker \
                  WHERE e.id = ?1 AND COALESCE(p.url,'') <> ''",
                [s],
                |r| r.get(0),
            )
            .ok();
        let host = host
            .as_deref()
            .and_then(|u| u.split("//").nth(1))
            .map(|h| h.trim_start_matches("www.").split('/').next().unwrap_or("").to_string())
            .filter(|h| !h.is_empty());

        let rows: Vec<(i64, String, String)> = {
            let mut q = con.prepare(
                "SELECT id, source_url, taken_at FROM docs \
                  WHERE subject = ?1 AND kind = 'description' \
                  ORDER BY taken_at DESC, source_url",
            )?;
            let v: Vec<(i64, String, String)> = q
                .query_map([s], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };
        if rows.len() < 2 {
            continue;
        }
        let keep = host
            .as_ref()
            .and_then(|h| rows.iter().find(|(_, u, _)| u.contains(h.as_str())))
            .unwrap_or(&rows[0])
            .0;
        for (id, _, _) in &rows {
            if *id != keep {
                dropped += 1;
                if apply {
                    con.execute("DELETE FROM docs WHERE id = ?1", [id])?;
                }
            }
        }
    }
    // A text that restates the thing's own name is not a description of it.
    // "Ultra speed version of gpt-oss-120b" reads on the card as though
    // nobody bothered, which is what the check already says about anything
    // under forty characters; this is that rule carried out rather than
    // reported every night.
    let echoes: Vec<i64> = {
        let mut q = con.prepare(
            "SELECT id FROM docs WHERE kind='description'               AND LENGTH(TRIM(text)) < 40 ORDER BY id",
        )?;
        let v: Vec<i64> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    for id in &echoes {
        dropped += 1;
        if apply {
            con.execute("DELETE FROM docs WHERE id = ?1", [id])?;
        }
    }
    println!(
        "things described more than once: {}, descriptions that only echo the name: {}, \
         descriptions to drop: {dropped}",
        subjects.len(),
        echoes.len()
    );
    Ok(dropped)
}

// ---------------------------------------------------------------------------
// The vocabulary
// ---------------------------------------------------------------------------

/// What the words on the rest of the site mean.
///
/// A catalogue of prices assumes its reader already knows what a token is,
/// what a context window costs them, and why one model is sold by the million
/// tokens and another by the second. Most do not.
///
/// The entries are data rather than code, kept in `data/terms.json` and
/// written into the catalogue whole every night, so a correction lands the
/// same night it is made. Each answers in its first sentence, then says why
/// it matters to somebody buying; `see` points at a page that shows the thing
/// rather than describing it, so the vocabulary is a way in and not a
/// cul-de-sac.
pub const TERMS_JSON: &str = include_str!("../data/terms.json");

const TERMS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS terms (
    slug   TEXT PRIMARY KEY,
    term   TEXT NOT NULL,
    kind   TEXT NOT NULL,
    short  TEXT NOT NULL,
    body   TEXT NOT NULL,
    also   TEXT NOT NULL DEFAULT '[]',
    see    TEXT NOT NULL DEFAULT '[]'
);
";

/// The two JSON columns are written by hand rather than by the serialiser.
/// serde_json orders an object's keys alphabetically and puts no space after
/// a comma; the Python this replaces keeps the order they were written in and
/// does. Both are valid JSON and no reader would notice, but the port is
/// accepted by comparing what lands in the table, and a difference nobody can
/// see is still a difference nobody explained.
fn json_string(v: &serde_json::Value) -> String {
    json_ascii(v.as_str().unwrap_or(""))
}

fn json_list(v: &serde_json::Value) -> String {
    let items: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(json_string)
        .collect();
    format!("[{}]", items.join(", "))
}

fn json_links(v: &serde_json::Value) -> String {
    let items: Vec<String> = v
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            format!(
                "{{\"label\": {}, \"href\": {}}}",
                json_string(&l["label"]),
                json_string(&l["href"])
            )
        })
        .collect();
    format!("[{}]", items.join(", "))
}

pub fn terms(con: &Connection, apply: bool) -> Result<usize> {
    con.execute_batch(TERMS_SCHEMA)?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(TERMS_JSON)?;
    println!("{} terms", rows.len());

    // In descending count, so the shape of the vocabulary is visible at a
    // glance; first-seen order settles a tie, as the Python's sort does.
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for r in &rows {
        let k = r["kind"].as_str().unwrap_or("").to_string();
        match kinds.iter_mut().find(|(x, _)| *x == k) {
            Some((_, n)) => *n += 1,
            None => kinds.push((k, 1)),
        }
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in &kinds {
        println!("   {k:<26} {n}");
    }
    if !apply {
        println!("\ndry run; nothing written");
        return Ok(rows.len());
    }
    // Written whole: a term removed from the file is a term that leaves the
    // page the same night, which is what makes the file the editing surface.
    con.execute("DELETE FROM terms", [])?;
    for r in &rows {
        con.execute(
            "INSERT INTO terms (slug,term,kind,short,body,also,see) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                r["slug"].as_str().unwrap_or(""),
                r["term"].as_str().unwrap_or(""),
                r["kind"].as_str().unwrap_or(""),
                r["short"].as_str().unwrap_or(""),
                r["body"].as_str().unwrap_or(""),
                json_list(&r["also"]),
                json_links(&r["see"]),
            ],
        )?;
    }
    println!("\nwrote {}", rows.len());
    Ok(rows.len())
}

// ---------------------------------------------------------------------------
// What somebody will run for you and charge you nothing
// ---------------------------------------------------------------------------

/// Not open weights. That you may download a model and host it yourself is a
/// fact about its licence. This is the other kind: somebody else's machine,
/// somebody else's bill, and a cap.
///
/// The cap is the whole entry. "Free" without it is not a fact a reader can
/// act on — thirty requests a minute and a thousand a day is generous or
/// useless depending on what you are building — so every row carries the
/// allowance as the seller printed it, and a tier whose limits the seller
/// does not publish is not recorded.
///
/// None of it can be crawled: one seller states its limits only inside a
/// signed-in console, another gives a credit figure and no rate at all. So it
/// is a table somebody filled in by reading, with the address beside every
/// line, kept in `data/free_tiers.json`.
pub const FREE_TIERS_JSON: &str = include_str!("../data/free_tiers.json");

pub fn free_tiers(con: &Connection, apply: bool) -> Result<(usize, usize)> {
    let doc: serde_json::Value = serde_json::from_str(FREE_TIERS_JSON)?;
    let lanes = doc["lanes"].as_array().cloned().unwrap_or_default();
    let plans = doc["plans"].as_array().cloned().unwrap_or_default();

    // The figures behind each sentence live in their own column. The server
    // adds it at startup; a tool should not have to wait for a deploy to be
    // able to write.
    let has: Option<i64> = con
        .query_row(
            "SELECT 1 FROM pragma_table_info('offerings') WHERE name='allowance'",
            [],
            |r| r.get(0),
        )
        .ok();
    if has.is_none() {
        con.execute("ALTER TABLE offerings ADD COLUMN allowance TEXT", [])?;
    }
    let today: String =
        con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;

    let mut by_name: HashMap<String, String> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            by_name.insert(name, id);
        }
    }

    let mut bound: Vec<(String, &serde_json::Value)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for l in &lanes {
        let model = l["model"].as_str().unwrap_or("").to_string();
        match by_name.get(&model) {
            Some(eid) => bound.push((eid.clone(), l)),
            None => missing.push(model),
        }
    }
    println!("free lanes in the table : {}", lanes.len());
    println!("  bound                 : {}", bound.len());
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        println!("  no such model here    : {}", missing.join(", "));
    }
    println!("free plans in the table : {}", plans.len());
    if !apply {
        println!("\ndry run; nothing written");
        return Ok((bound.len(), plans.len()));
    }

    // This job owns the free lanes it writes: an allowance that has been
    // withdrawn must stop being advertised, and the only way to know it was
    // withdrawn is that it is no longer in the table.
    let mut sellers: Vec<String> = lanes
        .iter()
        .map(|l| l["seller"].as_str().unwrap_or("").to_string())
        .collect();
    sellers.sort();
    sellers.dedup();
    for pid in &sellers {
        con.execute(
            "DELETE FROM prices WHERE offering_id IN \
             (SELECT id FROM offerings WHERE provider_id=?1 AND variant='free')",
            [pid],
        )?;
        con.execute(
            "DELETE FROM offerings WHERE provider_id=?1 AND variant='free'",
            [pid],
        )?;
    }

    let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
    for (eid, l) in &bound {
        let kind = s(&l["seller_kind"]);
        con.execute(
            "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,?3,?4)",
            rusqlite::params![s(&l["seller"]), s(&l["seller_name"]), s(&l["seller_url"]), kind],
        )?;
        con.execute(
            "INSERT INTO offerings (entity_id,provider_id,way,variant,limits,allowance,\
             status,first_seen,last_seen) VALUES (?1,?2,?3,'free',?4,?5,'live',?6,?6)",
            rusqlite::params![
                eid,
                s(&l["seller"]),
                if kind == "aggregator" { "aggregator" } else { "api" },
                s(&l["limits"]),
                json_object(&l["allowance"]),
                today
            ],
        )?;
        let oid = con.last_insert_rowid();
        for dim in ["mtok_in", "mtok_out"] {
            con.execute(
                "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,source_url,\
                 taken_at) VALUES (?1,?2,0,'declared',?3,?4)",
                rusqlite::params![oid, dim, s(&l["source_url"]), today],
            )?;
        }
    }

    for p in &plans {
        con.execute(
            "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,?3,'vendor')",
            rusqlite::params![s(&p["maker"]), s(&p["maker_name"]), s(&p["maker_url"])],
        )?;
        let grants = p["grants"].as_str().and_then(|g| by_name.get(g).cloned());
        let attrs = format!(
            "{{\"limits\": {}, \"includes\": []}}",
            json_ascii(&s(&p["limits"]))
        );
        con.execute(
            "INSERT INTO entities (id,register,name,maker,input_kind,output_kind,\
             derived_from,attrs) VALUES (?1,'subscription',?2,?3,'','',?4,?5) \
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, attrs=excluded.attrs",
            rusqlite::params![s(&p["id"]), s(&p["name"]), s(&p["maker"]), grants, attrs],
        )?;
        let existing: Option<i64> = con
            .query_row(
                "SELECT id FROM offerings WHERE entity_id=?1 AND provider_id=?2 \
                  AND variant='free'",
                (s(&p["id"]), s(&p["maker"])),
                |r| r.get(0),
            )
            .ok();
        let oid = match existing {
            Some(oid) => {
                con.execute(
                    "UPDATE offerings SET last_seen=?1, limits=?2, allowance=?3 WHERE id=?4",
                    rusqlite::params![today, s(&p["limits"]), json_object(&p["allowance"]), oid],
                )?;
                oid
            }
            None => {
                con.execute(
                    "INSERT INTO offerings (entity_id,provider_id,way,variant,limits,\
                     allowance,status,first_seen,last_seen) \
                     VALUES (?1,?2,'subscription','free',?3,?4,'live',?5,?5)",
                    rusqlite::params![
                        s(&p["id"]),
                        s(&p["maker"]),
                        s(&p["limits"]),
                        json_object(&p["allowance"]),
                        today
                    ],
                )?;
                con.last_insert_rowid()
            }
        };
        con.execute(
            "DELETE FROM prices WHERE offering_id=?1 AND dimension='month'",
            [oid],
        )?;
        con.execute(
            "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,source_url,\
             taken_at) VALUES (?1,'month',0,'declared',?2,?3)",
            rusqlite::params![oid, s(&p["source_url"]), today],
        )?;
    }
    println!(
        "\nwrote {} free lanes and {} free plans",
        bound.len(),
        plans.len()
    );
    Ok((bound.len(), plans.len()))
}

/// A JSON string in the Python's spelling. `json.dumps` escapes everything
/// above ASCII by default, so an em dash is written `\u2014`; serde writes the
/// character. Both parse to the same string and no reader could tell, and it
/// is still a difference in what lands in the column.
fn json_ascii(v: &str) -> String {
    let mut out = String::from("\"");
    for ch in v.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if c.is_ascii() => out.push(c),
            // Above the basic plane Python writes a surrogate pair, as JSON
            // has no other way to spell it.
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// The allowance column, in the Python's spelling: keys in the order written,
/// a space after each comma and colon. See `json_list` for why by hand.
fn json_object(v: &serde_json::Value) -> String {
    let Some(map) = v.as_object() else { return "{}".into() };
    let items: Vec<String> = map
        .iter()
        .map(|(k, val)| {
            format!(
                "{}: {}",
                serde_json::to_string(k).unwrap_or_default(),
                serde_json::to_string(val).unwrap_or_default()
            )
        })
        .collect();
    format!("{{{}}}", items.join(", "))
}

// ---------------------------------------------------------------------------
// Plans bought by the month, and what each allows
// ---------------------------------------------------------------------------

/// A seat price is not a rate card, and no feed publishes one. Somebody read
/// each page and wrote the figure down with the address beside it; the table
/// is re-applied nightly, so a plan whose price moved stops advertising the
/// old one the same night.
pub const SUBSCRIPTIONS_JSON: &str = include_str!("../data/subscriptions.json");

/// "Pro" and "Pro+" are different plans at different prices, and dropping the
/// plus gave them one id — the dearer silently replaced the cheaper.
fn plan_slug(name: &str) -> String {
    static R: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (non, runs) = R.get_or_init(|| {
        (Regex::new(r"[^a-z0-9]+").unwrap(), Regex::new(r"-{2,}").unwrap())
    });
    let lowered = name.to_lowercase().replace('+', " plus ");
    let hyphened = non.replace_all(&lowered, "-");
    runs.replace_all(hyphened.trim_matches('-'), "-").into_owned()
}

pub fn subscriptions(con: &Connection, apply: bool) -> Result<usize> {
    let plans: Vec<serde_json::Value> = serde_json::from_str(SUBSCRIPTIONS_JSON)?;
    let today: String =
        con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
    let mut grants: HashMap<String, String> = HashMap::new();
    {
        let mut q = con.prepare("SELECT id, name FROM entities")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            grants.insert(name, id);
        }
    }

    // An earlier draft hung these plans off the product as a way of buying
    // it. They are their own things now, and leaving both would price every
    // one of them twice.
    if apply {
        con.execute(
            "DELETE FROM prices WHERE offering_id IN \
             (SELECT o.id FROM offerings o JOIN entities e ON e.id=o.entity_id \
               WHERE o.way='subscription' AND e.register <> 'subscription')",
            [],
        )?;
        con.execute(
            "DELETE FROM offerings WHERE way='subscription' AND entity_id IN \
             (SELECT id FROM entities WHERE register <> 'subscription')",
            [],
        )?;
    }

    let s = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
    let (mut made, mut free) = (0usize, 0usize);
    for p in &plans {
        let plan = s(&p["plan"]);
        let eid = format!("sub_{}", plan_slug(&plan));
        let usd = p["usd"].as_f64().unwrap_or(0.0);
        if usd == 0.0 {
            free += 1;
        }
        let known: Option<i64> = con
            .query_row("SELECT 1 FROM entities WHERE id=?1", [&eid], |r| r.get(0))
            .ok();
        if known.is_none() {
            made += 1;
        }
        if !apply {
            continue;
        }
        con.execute(
            "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,?3,'vendor')",
            rusqlite::params![s(&p["maker"]), s(&p["maker_name"]), s(&p["maker_url"])],
        )?;
        let includes: Vec<String> = p["includes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| json_ascii(x.as_str().unwrap_or("")))
            .collect();
        let attrs = format!(
            "{{\"limits\": {}, \"includes\": [{}]}}",
            json_ascii(&s(&p["limits"])),
            includes.join(", ")
        );
        con.execute(
            "INSERT INTO entities (id,register,name,maker,input_kind,output_kind,\
             derived_from,attrs) VALUES (?1,'subscription',?2,?3,'','',?4,?5) \
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, maker=excluded.maker, \
                                           derived_from=excluded.derived_from, \
                                           attrs=excluded.attrs",
            rusqlite::params![
                &eid,
                &plan,
                s(&p["maker"]),
                p["grants"].as_str().and_then(|g| grants.get(g).cloned()),
                attrs
            ],
        )?;
        // A plan that costs nothing is still a plan; it goes on the free lane
        // so that the rule which keeps a gift out of a price range keeps this
        // one out too.
        let variant = if usd == 0.0 { "free" } else { "" };
        let existing: Option<i64> = con
            .query_row(
                "SELECT id FROM offerings WHERE entity_id=?1 AND provider_id=?2 \
                  AND variant=?3",
                rusqlite::params![&eid, s(&p["maker"]), variant],
                |r| r.get(0),
            )
            .ok();
        let oid = match existing {
            Some(oid) => {
                con.execute(
                    "UPDATE offerings SET last_seen=?1, limits=?2 WHERE id=?3",
                    rusqlite::params![today, s(&p["limits"]), oid],
                )?;
                oid
            }
            None => {
                con.execute(
                    "INSERT INTO offerings (entity_id,provider_id,way,variant,limits,\
                     status,first_seen,last_seen) \
                     VALUES (?1,?2,'subscription',?3,?4,'live',?5,?5)",
                    rusqlite::params![&eid, s(&p["maker"]), variant, s(&p["limits"]), today],
                )?;
                con.last_insert_rowid()
            }
        };
        con.execute(
            "DELETE FROM prices WHERE offering_id=?1 AND dimension='month'",
            [oid],
        )?;
        con.execute(
            "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,source_url,\
             taken_at) VALUES (?1,'month',?2,'declared',?3,?4)",
            rusqlite::params![oid, (usd * 1e6).round() as i64, s(&p["source_url"]), today],
        )?;
    }
    println!("plans in the table : {}  ({free} of them free)", plans.len());
    println!("  new to the catalogue: {made}");
    println!("\n{}", if apply { "written" } else { "dry run; nothing written" });
    Ok(plans.len())
}

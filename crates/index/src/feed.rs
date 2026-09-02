//! Where a collector's output becomes rows.
//!
//! The catalogue has two ways in, and they are not the same. `collector`
//! binds a listing only to an alias an operator has already minted and holds
//! everything else in the pen — deliberate, and right for a source nobody has
//! vetted. The collectors that read a known gateway's rate card go the other
//! way: they bind through the resolver, which reduces a seller's spelling
//! down to a name the catalogue already answers to, and that is what lets a
//! card say a model is sold by twenty-nine companies rather than by one.
//!
//! This is the second path. It exists because writing the gateways through
//! the first one parsed 546 listings correctly and wrote nothing at all.

use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One thing a source said, in the only shape the writer accepts. A collector
/// yields these and nothing else — no SQL, no counting, no reporting.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The name the source used, before binding.
    pub subject: String,
    pub source_url: String,
    /// The figures, in micro-dollars per unit, keyed by dimension.
    pub payload: Vec<(String, i64)>,
    /// The provider id this was read from.
    pub seller: String,
}

/// Persist price observations. The only place a collector's output turns into
/// rows, so the rules live once: the standard lane, never a nought unless the
/// seller declared it free, and the source recorded beside every figure.
pub fn write_prices(
    con: &rusqlite::Connection,
    obs: &[Observation],
    resolver: &mut crate::resolve::Resolver,
    today: &str,
    way: &str,
) -> Result<(usize, usize)> {
    let mut seen: HashMap<(String, String), i64> = HashMap::new();
    // What this run has already written, so a second alias of the same
    // seller landing on the same entity cannot silently overwrite the first:
    // the first figure stands and the clash is counted out loud.
    let mut written: HashMap<(i64, String), i64> = HashMap::new();
    let mut clashes = 0usize;
    let (mut bound, mut wrote) = (0usize, 0usize);
    for ob in obs {
        let Some(eid) = resolver.bind(&ob.subject) else { continue };
        bound += 1;
        let key = (eid.clone(), ob.seller.clone());
        let oid = match seen.get(&key) {
            Some(id) => *id,
            None => {
                let found: Option<i64> = con
                    .query_row(
                        "SELECT id FROM offerings WHERE entity_id=?1 AND provider_id=?2 \
                          AND COALESCE(variant,'')=''",
                        (&eid, &ob.seller),
                        |r| r.get(0),
                    )
                    .ok();
                let id = match found {
                    Some(id) => {
                        con.execute(
                            "UPDATE offerings SET last_seen=?1, status='live' WHERE id=?2",
                            (today, id),
                        )?;
                        id
                    }
                    None => {
                        con.execute(
                            "INSERT INTO offerings (entity_id,provider_id,way,variant,status,\
                             first_seen,last_seen) VALUES (?1,?2,?3,'','live',?4,?4)",
                            rusqlite::params![&eid, &ob.seller, way, today],
                        )?;
                        con.last_insert_rowid()
                    }
                };
                seen.insert(key, id);
                id
            }
        };
        for (dim, micros) in &ob.payload {
            // A nought here is a rounding mistake, not a gift; the declared
            // kind arrives on its own lane, called free.
            if *micros <= 0 {
                continue;
            }
            match written.get(&(oid, dim.clone())) {
                Some(before) if *before != *micros => {
                    clashes += 1;
                    continue;
                }
                Some(_) => continue,
                None => {
                    written.insert((oid, dim.clone()), *micros);
                }
            }
            con.execute(
                "DELETE FROM prices WHERE offering_id=?1 AND dimension=?2 AND source_url=?3",
                rusqlite::params![oid, dim, &ob.source_url],
            )?;
            con.execute(
                "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,source_url,\
                 taken_at) VALUES (?1,?2,?3,'declared',?4,?5)",
                rusqlite::params![oid, dim, micros, &ob.source_url, today],
            )?;
            wrote += 1;
        }
    }
    if clashes > 0 {
        println!("  {clashes} rows disagreed with an earlier alias of the same seller and were dropped");
    }
    Ok((bound, wrote))
}

// ---------------------------------------------------------------------------
// Whether the weights are published
// ---------------------------------------------------------------------------

/// models.dev carries `open_weights` on every model it lists, and the price
/// collector that reads the same document has been discarding it. That
/// silence is why "best open source" was unreliable: the pick was drawn only
/// from models whose licence somebody read off a card, and a third of a field
/// is not a field.
///
/// Two things this refuses to do. It does not invent a licence — "open
/// weights" is not a licence name, and writing apache-2.0 because a boolean
/// was true would put a specific claim on the card that nobody made. And it
/// does not believe one voice: some sellers mark other people's open weights
/// closed, so the sellers vote, a clear majority carries, and a tie leaves
/// the model exactly as unread as it was.
pub const MODELS_DEV: &str = "https://models.dev/api.json";
const CLOSED: &str = "proprietary";

pub struct Weights {
    pub stated: usize,
    pub already: usize,
    pub split: usize,
    pub opened: Vec<String>,
    pub closed: Vec<String>,
}

pub fn weigh(
    con: &rusqlite::Connection,
    doc: &serde_json::Value,
    resolver: &mut crate::resolve::Resolver,
) -> anyhow::Result<Weights> {
    let mut held: HashMap<String, (bool, bool)> = HashMap::new();
    {
        let mut q = con.prepare(
            "SELECT id, json_extract(attrs,'$.license'), \
                    json_extract(attrs,'$.open_weights') FROM entities",
        )?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let lic: Option<String> = r.get(1).ok().flatten();
            let ow: Option<rusqlite::types::Value> = r.get(2).ok();
            let has_ow = !matches!(ow, None | Some(rusqlite::types::Value::Null));
            held.insert(id, (lic.is_some(), has_ow));
        }
    }

    // Insertion order, so which of two equally-voted entities is reported
    // first does not move between runs.
    let mut order: Vec<String> = Vec::new();
    let mut votes: HashMap<String, (usize, usize)> = HashMap::new();
    if let Some(provs) = doc.as_object() {
        for (_prov, pv) in provs {
            let Some(models) = pv["models"].as_object() else { continue };
            for (mid, m) in models {
                let Some(ow) = m["open_weights"].as_bool() else { continue };
                let named = m["name"].as_str().unwrap_or(mid);
                let Some(eid) = resolver.bind(named).or_else(|| resolver.bind(mid)) else {
                    continue;
                };
                let e = votes.entry(eid.clone()).or_insert_with(|| {
                    order.push(eid.clone());
                    (0, 0)
                });
                if ow { e.0 += 1 } else { e.1 += 1 }
            }
        }
    }

    let mut out = Weights {
        stated: votes.len(),
        already: 0,
        split: 0,
        opened: Vec::new(),
        closed: Vec::new(),
    };
    for eid in &order {
        let (yes, no) = votes[eid];
        if yes == no {
            out.split += 1;
            continue;
        }
        let (has_lic, has_ow) = held.get(eid).copied().unwrap_or((false, false));
        if has_lic || has_ow {
            out.already += 1;
            continue;
        }
        if yes > no {
            out.opened.push(eid.clone());
        } else {
            out.closed.push(eid.clone());
        }
    }
    Ok(out)
}

pub fn write_weights(con: &rusqlite::Connection, w: &Weights) -> anyhow::Result<usize> {
    for eid in &w.opened {
        con.execute(
            "UPDATE entities SET attrs = json_set(coalesce(attrs,'{}'), \
             '$.open_weights', json('true'), '$.open_weights_source', ?1) WHERE id = ?2",
            (MODELS_DEV, eid),
        )?;
    }
    for eid in &w.closed {
        con.execute(
            "UPDATE entities SET attrs = json_set(coalesce(attrs,'{}'), '$.license', ?1) \
             WHERE id = ?2",
            (CLOSED, eid),
        )?;
    }
    Ok(w.opened.len() + w.closed.len())
}

// ---------------------------------------------------------------------------
// What one seller gives away
// ---------------------------------------------------------------------------

/// A price of nought is normally an error — a rate that rounded below the
/// smallest unit the catalogue stores — and the daily check blocks on it. A
/// nought is a fact only where the seller has said so in the name of the
/// thing: OpenRouter files these under an id ending `:free`. That declaration
/// is the whole evidence, so nothing without it is taken, and the offering
/// goes on a lane called `free`, which is what tells the check this nought
/// was meant.
pub const OPENROUTER_MODELS: &str = "https://openrouter.ai/api/v1/models";
pub const OPENROUTER: &str = "prov_openrouter";

pub struct Given {
    pub id: String,
    pub name: String,
    pub expires: Option<String>,
}

pub fn given_away(doc: &serde_json::Value) -> Vec<Given> {
    let mut out = Vec::new();
    for m in doc["data"].as_array().into_iter().flatten() {
        let Some(mid) = m["id"].as_str() else { continue };
        if !mid.ends_with(":free") {
            continue; // the seller's own declaration, or nothing
        }
        // Named free but billed is not our claim to make.
        let charged = |field: &str| -> bool {
            match &m["pricing"][field] {
                serde_json::Value::Number(n) => n.as_f64().unwrap_or(1.0) != 0.0,
                serde_json::Value::String(s) => s.parse::<f64>().unwrap_or(1.0) != 0.0,
                _ => true,
            }
        };
        if charged("prompt") || charged("completion") {
            continue;
        }
        out.push(Given {
            id: mid.to_string(),
            name: m["name"].as_str().unwrap_or(mid).to_string(),
            expires: m["expiration_date"].as_str().map(str::to_string),
        });
    }
    out
}

/// This job owns the free lane at this seller: a listing that has stopped
/// being free must stop being advertised as free, and the only way to know is
/// that it is no longer in the feed.
pub fn write_given(
    con: &rusqlite::Connection,
    bound: &[(String, &Given)],
    today: &str,
) -> anyhow::Result<usize> {
    // An empty read means the free feed failed to fetch, not that the seller
    // withdrew every free lane at once. Clearing on it would blank the free
    // offerings until the next good run; better to keep last night's than to
    // advertise nothing on a fetch blip.
    if bound.is_empty() {
        return Ok(0);
    }
    con.execute(
        "DELETE FROM prices WHERE offering_id IN \
         (SELECT id FROM offerings WHERE provider_id=?1 AND variant='free')",
        [OPENROUTER],
    )?;
    con.execute(
        "DELETE FROM offerings WHERE provider_id=?1 AND variant='free'",
        [OPENROUTER],
    )?;
    for (eid, o) in bound {
        let limits = match &o.expires {
            Some(e) => format!("free until {e}"),
            None => "no end date given".to_string(),
        };
        con.execute(
            "INSERT INTO offerings (entity_id,provider_id,way,variant,limits,status,\
             first_seen,last_seen) VALUES (?1,?2,'aggregator','free',?3,'live',?4,?4)",
            rusqlite::params![eid, OPENROUTER, limits, today],
        )?;
        let oid = con.last_insert_rowid();
        for dim in ["mtok_in", "mtok_out"] {
            con.execute(
                "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,source_url,\
                 taken_at) VALUES (?1,?2,0,'declared',?3,?4)",
                rusqlite::params![oid, dim, OPENROUTER_MODELS, today],
            )?;
        }
    }
    Ok(bound.len())
}

// ---------------------------------------------------------------------------
// What a thing is, beyond what it costs
// ---------------------------------------------------------------------------

/// When it came out, what it was trained up to, how much it will read and
/// write back, whether it reasons and whether it calls tools — from the same
/// two feeds the price collectors read and throw away.
///
/// Each fact is settled by the rule its own nature demands rather than one
/// rule for six. A date is the earliest anybody gave, floored so that a model
/// cannot be published before the material it was trained on; a size is the
/// largest, because a seller who caps a context is describing their own
/// serving and not the model; everything else is a majority, and no majority
/// leaves the fact unread.
pub struct Said {
    pub fact: &'static str,
    pub entity: String,
    pub value: serde_json::Value,
}

/// One shape for a date, whatever shape it arrived in.
pub fn as_date(v: &serde_json::Value) -> Option<String> {
    if let Some(n) = v.as_f64() {
        if n > 1_000_000_000.0 {
            // Seconds since the epoch, turned into a plain day. Written out
            // because the crate has no date library and one line of civil
            // arithmetic is cheaper than one.
            return Some(epoch_day(n as i64));
        }
    }
    if let Some(s) = v.as_str() {
        let b = s.as_bytes();
        if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' {
            return Some(s[..10].to_string());
        }
    }
    None
}

/// The civil date of a UTC timestamp, by Howard Hinnant's days-from-civil in
/// reverse — the standard algorithm, and short enough to read.
fn epoch_day(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// fact -> entity -> [(value, who said it)], in the order the feeds were read.
pub type Heard = Vec<(&'static str, String, serde_json::Value)>;

pub fn hear_facts(
    openrouter: &serde_json::Value,
    modelsdev: &serde_json::Value,
    r: &mut crate::resolve::Resolver,
) -> Heard {
    let mut say: Heard = Vec::new();
    let mut note = |say: &mut Heard, eid: &str, fact: &'static str, v: serde_json::Value| {
        let empty = v.is_null()
            || v.as_str() == Some("")
            || v.as_array().is_some_and(|a| a.is_empty())
            || v.as_object().is_some_and(|o| o.is_empty());
        if !empty {
            say.push((fact, eid.to_string(), v));
        }
    };

    for m in openrouter["data"].as_array().into_iter().flatten() {
        let named = m["name"].as_str().unwrap_or("");
        let id = m["id"].as_str().unwrap_or("");
        let Some(eid) = r.bind(named).or_else(|| r.bind(id)) else { continue };
        if let Some(d) = as_date(&m["created"]) {
            note(&mut say, &eid, "released", serde_json::json!(d));
        }
        note(&mut say, &eid, "context", m["context_length"].clone());
        note(&mut say, &eid, "max_output", m["top_provider"]["max_completion_tokens"].clone());
    }

    if let Some(provs) = modelsdev.as_object() {
        for (_prov, pv) in provs {
            let Some(models) = pv["models"].as_object() else { continue };
            for (mid, mv) in models {
                let named = mv["name"].as_str().unwrap_or(mid);
                let Some(eid) = r.bind(named).or_else(|| r.bind(mid)) else { continue };
                if let Some(d) = as_date(&mv["release_date"]) {
                    note(&mut say, &eid, "released", serde_json::json!(d));
                }
                note(&mut say, &eid, "context", mv["limit"]["context"].clone());
                note(&mut say, &eid, "max_output", mv["limit"]["output"].clone());
                let k = as_date(&mv["knowledge"])
                    .map(|d| serde_json::json!(d))
                    .unwrap_or_else(|| match mv["knowledge"].as_str() {
                        Some(s) => serde_json::json!(s),
                        None => serde_json::Value::Null,
                    });
                note(&mut say, &eid, "knowledge", k);
                note(&mut say, &eid, "reasoning", mv["reasoning"].clone());
                note(&mut say, &eid, "tool_call", mv["tool_call"].clone());
            }
        }
    }
    say
}

/// One value from many voices, by the rule this fact answers to.
pub fn settle(
    fact: &str,
    heard: &[serde_json::Value],
    floor: Option<&str>,
) -> Option<serde_json::Value> {
    if fact == "released" {
        // A model cannot be published before the material it was trained on.
        // Taking the earliest date any seller gave let one mislabelled listing
        // drag a 2025 model back to January 2024, and the page then printed
        // "published in January 2024, trained on material up to July 2024".
        let mut dates: Vec<&str> = heard.iter().filter_map(|v| v.as_str()).collect();
        if let Some(f) = floor {
            dates.retain(|d| *d >= f);
        }
        return dates.into_iter().min().map(|d| serde_json::json!(d));
    }
    if fact == "context" || fact == "max_output" {
        return heard
            .iter()
            .filter_map(|v| v.as_f64())
            .filter(|n| *n > 0.0)
            .max_by(|a, b| a.total_cmp(b))
            .map(|n| serde_json::json!(n as i64));
    }
    // First-seen order among equal counts, so a tie in the tally is settled
    // the way Python's Counter settles it.
    let mut tally: Vec<(String, usize)> = Vec::new();
    for v in heard {
        let key = canonical(v);
        match tally.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => tally.push((key, 1)),
        }
    }
    tally.sort_by(|a, b| b.1.cmp(&a.1));
    let (top, n) = tally.first()?.clone();
    let rest: usize = tally.iter().skip(1).map(|(_, c)| c).sum();
    if rest >= n {
        return None; // no majority: leave it unread
    }
    serde_json::from_str(&top).ok()
}

/// The key a tally counts by: the same value written the same way whatever
/// order its keys arrived in.
fn canonical(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| format!("{}: {}", serde_json::to_string(k).unwrap_or_default(), canonical(&m[*k])))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        serde_json::Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(canonical).collect();
            format!("[{}]", parts.join(", "))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Models the market sells that the catalogue does not hold
// ---------------------------------------------------------------------------

/// A name earns an entity only when every marker of how it is served comes
/// off and it still matches nothing here, and only when the feed that named
/// it also priced it. A card holding a name and nothing else is not an entry.
struct NewPats {
    serving: Regex,
    dated: Regex,
    leading_maker: Regex,
    at_region: Regex,
    short_date: Regex,
    unclosed: Regex,
    a_router: Regex,
    plan_prefix: Regex,
    mmdd: Regex,
    label: Regex,
    non_key: Regex,
}

fn new_pats() -> &'static NewPats {
    static P: OnceLock<NewPats> = OnceLock::new();
    P.get_or_init(|| {
        let vendors = crate::resolve::VENDORS.join("|");
        NewPats {
            // Built by concatenation: inside a raw string a backslash before
            // a newline is a backslash, not a continuation, and the pattern
            // silently carried one — so "DeepSeek R1 Fast" kept its lane and
            // twenty models were minted that are ways of serving one.
            serving: Regex::new(concat!(
                r"(?i)[\s._/-]*\(?\b(",
                r"fp4|fp8|fp16|bf16|int4|int8|mxfp4|nvfp4|awq|gptq|q4|q8|",
                r"fast|highspeed|high-speed|turbo|nitro|turbo-preview|priority|batch|flex|sandbox|",
                r"latest|preview|exp|experimental|beta|alpha|snapshot|stable|",
                r"free|trial|deprecated|legacy|online|thinking|reasoning|instruct|chat",
                r")\b\)?\s*$",
            ))
            .unwrap(),
            dated: Regex::new(
                r"[\s._/-]*(\((20\d{6}|20\d\d-\d\d-\d\d|\d{2}[-/]\d{2,4}|\d{4})\)|\b(20\d{6}|20\d\d-\d\d-\d\d)\b)\s*$",
            ).unwrap(),
            leading_maker: Regex::new(&format!(r"(?i)^({vendors})[\s._/-]+")).unwrap(),
            at_region: Regex::new(r"@[\w-]+\s*$").unwrap(),
            short_date: Regex::new(r"[-_](2[0-9](0[1-9]|1[0-2]))\s*$").unwrap(),
            unclosed: Regex::new(r"\s*\([^)]*$").unwrap(),
            a_router: Regex::new(r"(?i)\b(router|auto\s*router|routing)\b").unwrap(),
            plan_prefix: Regex::new(r"(?i)^(coding|agentic|chat|search|vision)\s+").unwrap(),
            mmdd: Regex::new(r"\s+(0[1-9]|1[0-2])([0-2][0-9]|3[01])\s*$").unwrap(),
            label: Regex::new(r"^[A-Za-z0-9 .&-]{1,22}:\s+").unwrap(),
            non_key: Regex::new(r"[^a-z0-9.]+").unwrap(),
        }
    })
}

/// The name with every marker of how it is served taken off.
pub fn bare(name: &str) -> String {
    let p = new_pats();
    let mut s = name.trim().to_string();
    if s.contains('/') && !s.contains(' ') {
        s = s.rsplit('/').next().unwrap_or(&s).to_string();
    }
    // "Qwen: QvQ Max" is a feed naming the maker before the model.
    s = p.label.replace(&s, "").into_owned();
    s = s.trim_matches([' ', '-', '–', '—', '/', '|']).to_string();
    s = p.unclosed.replace(&s, "").into_owned();
    s = p.plan_prefix.replace(&s, "").into_owned();
    s = p.mmdd.replace(&s, "").into_owned();
    s = p.at_region.replace(&s, "").into_owned();
    s = p.short_date.replace(&s, "").into_owned();
    for _ in 0..6 {
        let before = s.clone();
        s = p.serving.replace(&s, "").into_owned();
        s = p.dated.replace(&s, "").into_owned();
        s = s.trim_matches([' ', '-', '_', '/', '.', '(', ')']).to_string();
        // The trim above takes the closing bracket off "Gemma 4 26B (DeepInfra)"
        // and leaves the opening one behind, so the rule that removes an
        // unclosed bracket has to run after it and not only before. Nineteen
        // models were minted under a name ending mid-bracket, five of them
        // crediting the gateway that resold them as the maker.
        s = p.unclosed.replace(&s, "").into_owned();
        if s == before {
            break;
        }
    }
    s
}

/// One priced name the feeds carry that the catalogue cannot place.
pub struct Unplaced {
    pub key: String,
    pub name: String,
    pub raw: String,
    pub maker: String,
    pub sellers: Vec<String>,
    pub blob: serde_json::Value,
    pub quotes: Vec<(String, Vec<(String, i64)>)>,
}

pub fn unplaced(
    modelsdev: &serde_json::Value,
    modelsdev_raw: &str,
    openrouter: &serde_json::Value,
    r: &mut crate::resolve::Resolver,
    companies: &[String],
) -> Vec<Unplaced> {
    let p = new_pats();
    let mut order: Vec<String> = Vec::new();
    let mut out: HashMap<String, Unplaced> = HashMap::new();

    let mut note = |out: &mut HashMap<String, Unplaced>,
                    order: &mut Vec<String>,
                    r: &mut crate::resolve::Resolver,
                    name: &str,
                    maker: &str,
                    blob: &serde_json::Value,
                    seller: &str,
                    price: Vec<(String, i64)>| {
        if name.is_empty() || r.bind(name).is_some() {
            return;
        }
        let stripped = bare(name);
        let key = crate::resolve::norm(&p.leading_maker.replace(&stripped, ""));
        // A bare company name, or what is left of one after the strip, is not
        // a model. Nor is a fragment too short to identify anything.
        if key.len() < 4 || companies.contains(&key) || p.a_router.is_match(name) {
            return;
        }
        let row = out.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Unplaced {
                key: key.clone(),
                name: stripped.clone(),
                raw: name.to_string(),
                maker: maker.to_string(),
                sellers: Vec::new(),
                blob: blob.clone(),
                quotes: Vec::new(),
            }
        });
        if !row.sellers.contains(&seller.to_string()) {
            row.sellers.push(seller.to_string());
        }
        if !price.is_empty() {
            row.quotes.push((seller.to_string(), price));
        }
        if !maker.is_empty() && row.maker.is_empty() {
            row.maker = maker.to_string();
        }
    };

    for prov in keys_in_order(modelsdev_raw, &[]) {
        let pv = &modelsdev[&prov];
        {
            for mid in keys_in_order(modelsdev_raw, &[&prov, "models"]) {
                let mv = &pv["models"][&mid];
                let cost = &mv["cost"];
                let priced = ["input", "output"]
                    .iter()
                    .any(|k| cost[*k].as_f64().is_some_and(|v| v > 0.0));
                if !priced {
                    continue;
                }
                let named = mv["name"].as_str().unwrap_or(&mid);
                if r.bind(named).is_some() || r.bind(&mid).is_some() {
                    continue;
                }
                let maker = if mid.contains('/') {
                    mid.split('/').next().unwrap_or("")
                } else {
                    ""
                };
                // Dollars per million tokens, as models.dev states them.
                let mut px = Vec::new();
                for (k, d) in [
                    ("input", "mtok_in"),
                    ("output", "mtok_out"),
                    ("cache_read", "mtok_cache_read"),
                ] {
                    if let Some(v) = cost[k].as_f64().filter(|v| *v > 0.0) {
                        px.push((d.to_string(), (v * 1e6).round() as i64));
                    }
                }
                note(&mut out, &mut order, r, named, maker, mv, &prov, px);
            }
        }
    }

    for m in openrouter["data"].as_array().into_iter().flatten() {
        let mid = m["id"].as_str().unwrap_or("");
        let named = m["name"].as_str().unwrap_or("");
        if r.bind(named).is_some() || r.bind(mid).is_some() {
            continue;
        }
        let maker = if mid.contains('/') {
            mid.split('/').next().unwrap_or("")
        } else {
            ""
        };
        // Dollars per token, as OpenRouter states them.
        let mut px = Vec::new();
        for (k, d) in [("prompt", "mtok_in"), ("completion", "mtok_out")] {
            let v = match &m["pricing"][k] {
                serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
                serde_json::Value::String(s) => s.parse().unwrap_or(0.0),
                _ => 0.0,
            };
            if v > 0.0 {
                px.push((d.to_string(), (v * 1e12).round() as i64));
            }
        }
        let name = if named.is_empty() { mid } else { named };
        note(&mut out, &mut order, r, name, maker, m, "openrouter", px);
    }

    // A name whose stripped form now lands on something we hold was a way of
    // serving it, not a model. Checked after the strip, never before.
    order
        .into_iter()
        .filter_map(|k| out.remove(&k))
        .filter(|u| r.bind(&u.name).is_none())
        .collect()
}

/// Only what the feed said, in the catalogue's own fields.
pub fn new_facts(blob: &serde_json::Value) -> (String, String, Vec<(&'static str, serde_json::Value)>) {
    // A feed saying "pdf" means a file, and a modality nothing else in here
    // uses is a row no filter will ever find.
    fn say(x: &str) -> &str {
        match x {
            "pdf" | "document" => "file",
            "img" => "image",
            "speech" => "audio",
            other => other,
        }
    }
    const KNOWN: &[&str] = &["text", "image", "audio", "video", "file", "embedding"];
    let words = |v: &serde_json::Value| -> String {
        let mut out: Vec<String> = Vec::new();
        for x in v.as_array().into_iter().flatten() {
            let lowered = x.as_str().unwrap_or("").to_lowercase();
            let w = say(&lowered).to_string();
            if KNOWN.contains(&w.as_str()) && !out.contains(&w) {
                out.push(w);
            }
        }
        out.join(" + ")
    };
    let arch = &blob["architecture"];
    let mod_ = &blob["modalities"];
    let pick = |a: &serde_json::Value, b: &serde_json::Value| -> String {
        let first = words(a);
        if first.is_empty() { words(b) } else { first }
    };
    let takes = pick(&arch["input_modalities"], &mod_["input"]);
    let gives = pick(&arch["output_modalities"], &mod_["output"]);

    let mut attrs: Vec<(&'static str, serde_json::Value)> = Vec::new();
    let ctx = blob["context_length"]
        .as_i64()
        .or_else(|| blob["limit"]["context"].as_i64());
    if let Some(c) = ctx.filter(|c| *c > 0) {
        attrs.push(("context", serde_json::json!(c)));
    }
    if blob["open_weights"] == serde_json::Value::Bool(true) {
        attrs.push(("open_weights", serde_json::json!(true)));
    }
    if let Some(d) = blob["release_date"].as_str() {
        attrs.push(("released", serde_json::json!(&d[..d.len().min(10)])));
    }
    (
        if takes.is_empty() { "text".into() } else { takes },
        if gives.is_empty() { "text".into() } else { gives },
        attrs,
    )
}

/// The id a newly minted thing takes.
pub fn mint_id(name: &str) -> String {
    let p = new_pats();
    format!(
        "ent_{}",
        p.non_key
            .replace_all(&name.to_lowercase(), "-")
            .trim_matches('-')
    )
}

// ---------------------------------------------------------------------------
// The two public price files
// ---------------------------------------------------------------------------

/// LiteLLM's price file and models.dev price the same models the crawlers do,
/// from sellers who publish no list of their own. They add little stock —
/// seven names in ten are a model already here wearing another seller's
/// barcode — but they add the sellers, which is the whole point.
pub const LITELLM: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/\
model_prices_and_context_window.json";

/// The sellers we chose, and what each is called here. A price file names
/// hundreds of providers; a catalogue that took them all would be a list of
/// everybody's resellers rather than a market.
pub const SELLERS_JSON: &str = include_str!("../data/sellers.json");

/// The modes for which "per second" means a second of the thing produced or
/// heard. Everything else filling that field is charging for a second of
/// rented machine, which is a different market and belongs on no model card.
const BY_THE_SECOND: &[&str] =
    &["audio_transcription", "audio_speech", "video_generation", "realtime"];

/// What LiteLLM publishes and what each field means here. Three things this
/// table encodes that a loop could not: the multiplier, because a token price
/// is dollars per token and an image price is dollars per image and getting
/// it wrong by a factor of a million looks like a plausible number; the lane,
/// because batch and priority and flex are separate ways to buy and not
/// discounts on one price; and that in and out are two dimensions, because
/// folding them onto one "per second" would silently keep whichever was
/// written last.
const LITELLM_FIELDS: &[(&str, &str, &str, f64)] = &[
    ("input_cost_per_token", "", "mtok_in", PER_TOKEN),
    ("output_cost_per_token", "", "mtok_out", PER_TOKEN),
    ("cache_read_input_token_cost", "", "mtok_cache_read", PER_TOKEN),
    ("cache_creation_input_token_cost", "", "mtok_cache_write", PER_TOKEN),
    ("input_cost_per_audio_token", "", "mtok_in_audio", PER_TOKEN),
    ("output_cost_per_audio_token", "", "mtok_out_audio", PER_TOKEN),
    ("output_cost_per_reasoning_token", "", "mtok_out_reasoning", PER_TOKEN),
    ("output_cost_per_image", "", "image", PER_UNIT),
    ("input_cost_per_image", "", "image_in", PER_UNIT),
    ("input_cost_per_second", "", "second_in", PER_UNIT),
    ("output_cost_per_second", "", "second_out", PER_UNIT),
    ("input_cost_per_token_batches", "batch", "mtok_in", PER_TOKEN),
    ("output_cost_per_token_batches", "batch", "mtok_out", PER_TOKEN),
    ("input_cost_per_token_priority", "priority", "mtok_in", PER_TOKEN),
    ("output_cost_per_token_priority", "priority", "mtok_out", PER_TOKEN),
    ("cache_read_input_token_cost_priority", "priority", "mtok_cache_read", PER_TOKEN),
    ("input_cost_per_token_flex", "flex", "mtok_in", PER_TOKEN),
    ("output_cost_per_token_flex", "flex", "mtok_out", PER_TOKEN),
    ("input_cost_per_token_above_200k_tokens", "long-context", "mtok_in", PER_TOKEN),
    ("output_cost_per_token_above_200k_tokens", "long-context", "mtok_out", PER_TOKEN),
    ("cache_read_input_token_cost_above_200k_tokens", "long-context", "mtok_cache_read", PER_TOKEN),
];

const PER_TOKEN: f64 = 1e12;
const PER_UNIT: f64 = 1e6;

/// One (seller, model name, lane, figures) a price file carries.
pub struct Offer {
    pub seller: String,
    pub seller_name: String,
    pub kind: String,
    pub url: String,
    pub name: String,
    pub lane: String,
    pub px: Vec<(String, i64)>,
    pub src: &'static str,
}

/// Both feeds are read in the order they were written, for the same reason
/// the mint is: where two rows price one dimension from one source, the last
/// read wins, and "last" has to mean the same thing in both languages.
pub fn price_files(
    litellm: &serde_json::Value,
    litellm_raw: &str,
    modelsdev: &serde_json::Value,
    modelsdev_raw: &str,
) -> Vec<Offer> {
    let majors: serde_json::Value = serde_json::from_str(SELLERS_JSON).unwrap_or_default();
    let chosen = |key: &str| -> Option<(String, String, String, String)> {
        let m = majors.get(key)?;
        Some((
            m["id"].as_str()?.to_string(),
            m["name"].as_str()?.to_string(),
            m["kind"].as_str()?.to_string(),
            m["url"].as_str().unwrap_or("").to_string(),
        ))
    };
    let mut out = Vec::new();

    {
        for key in keys_in_order(litellm_raw, &[]) {
            let v = &litellm[&key];
            if key == "sample_spec" || !v.is_object() {
                continue;
            }
            let Some((pid, pname, kind, url)) =
                v["litellm_provider"].as_str().and_then(chosen)
            else {
                continue;
            };
            // A reserved-capacity contract is not a rate for the work. A
            // commitment row prices a second of wall clock on a machine
            // rented for a month, in the same field an audio model uses for
            // a second of speech; read literally it made one model cost six
            // milli-dollars per second of audio.
            if key.contains("-commitment/") {
                continue;
            }
            let mode = v["mode"].as_str().unwrap_or("");
            // Lanes in the order the table lists them, so two runs write the
            // same rows in the same order.
            let mut lanes: Vec<(String, Vec<(String, i64)>)> = Vec::new();
            for (src, lane, dim, mult) in LITELLM_FIELDS {
                let Some(c) = v[*src].as_f64().filter(|c| *c > 0.0) else { continue };
                if (*dim == "second_in" || *dim == "second_out")
                    && !BY_THE_SECOND.contains(&mode)
                {
                    continue;
                }
                let micros = (c * mult).round() as i64;
                match lanes.iter_mut().find(|(l, _)| l == lane) {
                    Some((_, px)) => px.push((dim.to_string(), micros)),
                    None => lanes.push((lane.to_string(), vec![(dim.to_string(), micros)])),
                }
            }
            for (lane, px) in lanes {
                out.push(Offer {
                    seller: pid.clone(),
                    seller_name: pname.clone(),
                    kind: kind.clone(),
                    url: url.clone(),
                    name: key.rsplit('/').next().unwrap_or(&key).to_string(),
                    lane,
                    px,
                    src: LITELLM,
                });
            }
        }
    }

    {
        for pkey in keys_in_order(modelsdev_raw, &[]) {
            let Some((pid, pname, kind, url)) = chosen(&pkey) else { continue };
            for mid in keys_in_order(modelsdev_raw, &[&pkey, "models"]) {
                let mv = &modelsdev[&pkey]["models"][&mid];
                let mut px = Vec::new();
                // Dollars per million tokens, as models.dev states them.
                for (src, dim) in [
                    ("input", "mtok_in"),
                    ("output", "mtok_out"),
                    ("cache_read", "mtok_cache_read"),
                    ("cache_write", "mtok_cache_write"),
                ] {
                    if let Some(c) = mv["cost"][src].as_f64().filter(|c| *c > 0.0) {
                        px.push((dim.to_string(), (c * 1e6).round() as i64));
                    }
                }
                out.push(Offer {
                    seller: pid.clone(),
                    seller_name: pname.clone(),
                    kind: kind.clone(),
                    url: url.clone(),
                    name: mv["name"].as_str().unwrap_or(&mid).to_string(),
                    lane: String::new(),
                    px,
                    src: MODELS_DEV,
                });
            }
        }
    }
    out
}

/// A seller's kind decides how the catalogue says it is bought.
pub fn way_of(kind: &str) -> &'static str {
    match kind {
        "cloud" => "cloud",
        "aggregator" => "aggregator",
        _ => "api",
    }
}

// ---------------------------------------------------------------------------
// The order a feed was written in
// ---------------------------------------------------------------------------

/// The keys of a JSON object, in the order the document lists them.
///
/// `serde_json` sorts an object's keys, and for most purposes that is fine.
/// Here it is not: a feed lists the same model under several providers, the
/// first one read decides which description the catalogue keeps, and which
/// description it keeps decides whether the name binds at all. Alphabetical
/// order minted twenty models the document order does not.
///
/// So the values come from the parsed document and the order comes from the
/// text. `path` names the object to look inside — empty for the top level.
pub fn keys_in_order(raw: &str, path: &[&str]) -> Vec<String> {
    let b = raw.as_bytes();
    let mut i = 0usize;
    let mut depth: i32 = -1; // -1 until the first `{` is entered
    let mut want: i32 = 0;
    let mut trail: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;

    while i < b.len() {
        match b[i] {
            b'"' => {
                let (s, next) = read_string(b, i);
                i = next;
                // A key is a string followed by a colon.
                let mut j = i;
                while j < b.len() && (b[j] as char).is_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == b':' {
                    if depth == want && trail.len() == path.len() {
                        out.push(s.clone());
                    }
                    pending = Some(s);
                    i = j + 1;
                }
            }
            b'{' | b'[' => {
                if b[i] == b'{' {
                    depth += 1;
                    // Entering the object whose key continues the path.
                    if trail.len() < path.len() {
                        if pending.as_deref() == Some(path[trail.len()]) {
                            trail.push(pending.clone().unwrap_or_default());
                            want = depth;
                        }
                    }
                } else {
                    depth += 1;
                }
                pending = None;
                i += 1;
            }
            b'}' | b']' => {
                if b[i] == b'}' && trail.len() == path.len() && depth == want && !path.is_empty() {
                    // Left the object asked for; nothing after it can add.
                    return out;
                }
                depth -= 1;
                pending = None;
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

/// A JSON string starting at `i`, and the index just past its closing quote.
fn read_string(b: &[u8], i: usize) -> (String, usize) {
    let mut j = i + 1;
    let mut s = String::new();
    while j < b.len() {
        match b[j] {
            b'\\' => {
                // The escape is kept verbatim; a key with one is rare and its
                // exact text is only used to match against the parsed key.
                if j + 1 < b.len() {
                    s.push(b[j] as char);
                    s.push(b[j + 1] as char);
                }
                j += 2;
            }
            b'"' => return (unescape(&s), j + 1),
            c => {
                s.push(c as char);
                j += 1;
            }
        }
    }
    (unescape(&s), j)
}

fn unescape(s: &str) -> String {
    serde_json::from_str::<String>(&format!("\"{s}\"")).unwrap_or_else(|_| s.to_string())
}

#[cfg(test)]
mod order_tests {
    use super::*;

    /// The whole point: the document's order, not the alphabet's.
    #[test]
    fn keys_come_back_in_the_order_written() {
        let raw = r#"{"zeta": {"models": {"b": 1, "a": 2}}, "alpha": {"models": {}}}"#;
        assert_eq!(keys_in_order(raw, &[]), vec!["zeta", "alpha"]);
        assert_eq!(keys_in_order(raw, &["zeta", "models"]), vec!["b", "a"]);
        assert_eq!(keys_in_order(raw, &["alpha", "models"]), Vec::<String>::new());
    }

    /// A brace or a colon inside a string is not structure.
    #[test]
    fn punctuation_inside_a_string_is_text() {
        let raw = r#"{"a": "}{: not a key", "b": {"c": 1}}"#;
        assert_eq!(keys_in_order(raw, &[]), vec!["a", "b"]);
        assert_eq!(keys_in_order(raw, &["b"]), vec!["c"]);
    }
}

//! Pass Index — the catalogue store (Stage 1).
//!
//! Six tables, one owner: this crate. Spec: `docs/specs/pass-index-data-model.md`.
//! Every price and metric row carries its provenance (source + date, NOT NULL)
//! and its basis (declared | measured); v1 writes only `declared`.

pub mod about;
pub mod intro;
pub mod top;

use std::collections::HashMap;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub mod collector;
pub mod boards;
pub mod checks;
pub mod feed;
pub mod hands;
pub mod supply;
pub mod prose;
pub mod repair;
pub mod resolve;
pub mod walk;

/// The catalogue schema, applied once per database file.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS providers (
    id    TEXT PRIMARY KEY,
    name  TEXT NOT NULL,
    url   TEXT,
    kind  TEXT,
    notes TEXT,
    -- Filled by the prose jobs (startups/funds/yc). Declared here so a fresh
    -- database carries them from the first night; the jobs used to ADD them
    -- by migration, which left a restored or rebuilt db a schema behind.
    raised        INTEGER,
    rounds        INTEGER,
    raised_source TEXT,
    founded       TEXT,
    listed        INTEGER,
    backing       TEXT,
    status        TEXT
);
CREATE TABLE IF NOT EXISTS entities (
    id           TEXT PRIMARY KEY,
    -- A subscription is a fourth kind of thing, not a way of buying the
    -- other three: it is sold by the month with a cap on it, and the cap is
    -- the fact a buyer needs. It has nowhere to live on a price list.
    register     TEXT NOT NULL CHECK (register IN ('model','tool','agent','subscription')),
    name         TEXT NOT NULL,
    maker        TEXT REFERENCES providers(id),
    family       TEXT,
    version      TEXT,
    derived_from TEXT REFERENCES entities(id),
    input_kind   TEXT NOT NULL,
    output_kind  TEXT NOT NULL,
    attrs        TEXT NOT NULL DEFAULT '{}'
);
-- What the last run of each self-check found. The coverage page shows these
-- rather than re-running anything: a mark a reader trusts has to be the
-- outcome of the same run that would have failed the deploy, and a check
-- that cannot report a failure is decoration.
CREATE TABLE IF NOT EXISTS checks (
    name     TEXT PRIMARY KEY,
    suite    TEXT NOT NULL,               -- which harness ran it
    blocking INTEGER NOT NULL DEFAULT 0,  -- does a finding stop the deploy
    findings INTEGER NOT NULL DEFAULT 0,  -- 0 is a pass
    asks     TEXT NOT NULL DEFAULT '',    -- the question, in words
    ran_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS aliases (
    source    TEXT NOT NULL,
    alias     TEXT NOT NULL,
    entity_id TEXT NOT NULL REFERENCES entities(id),
    PRIMARY KEY (source, alias)
);
-- variant is '' rather than NULL: SQLite treats NULLs as distinct in UNIQUE,
-- which would let the same offering exist twice.
CREATE TABLE IF NOT EXISTS offerings (
    id          INTEGER PRIMARY KEY,
    entity_id   TEXT NOT NULL REFERENCES entities(id),
    provider_id TEXT NOT NULL REFERENCES providers(id),
    way         TEXT NOT NULL CHECK (way IN ('api','aggregator','cloud','local','subscription','mcp')),
    variant     TEXT NOT NULL DEFAULT '',
    limits      TEXT,
    allowance   TEXT,
    status      TEXT NOT NULL DEFAULT 'live' CHECK (status IN ('live','stale','withdrawn')),
    first_seen  TEXT NOT NULL,
    last_seen   TEXT NOT NULL,
    UNIQUE (entity_id, provider_id, way, variant)
);
CREATE TABLE IF NOT EXISTS prices (
    id              INTEGER PRIMARY KEY,
    offering_id     INTEGER NOT NULL REFERENCES offerings(id),
    dimension       TEXT NOT NULL,
    micros_per_unit INTEGER NOT NULL,
    basis           TEXT NOT NULL DEFAULT 'declared' CHECK (basis IN ('declared','measured')),
    source_url      TEXT NOT NULL,
    taken_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS prices_by_offering ON prices(offering_id, dimension, id);
CREATE TABLE IF NOT EXISTS metrics (
    id          INTEGER PRIMARY KEY,
    offering_id INTEGER NOT NULL REFERENCES offerings(id),
    metric      TEXT NOT NULL,
    value       REAL NOT NULL,
    basis       TEXT NOT NULL DEFAULT 'declared' CHECK (basis IN ('declared','measured')),
    source_url  TEXT NOT NULL,
    taken_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS metrics_by_offering ON metrics(offering_id, metric, id);
-- What an independent measurer published about an entity. Quality belongs to
-- the weights, not to whoever serves them, so a score hangs on the entity
-- while speed stays on the offering. Append-only, same provenance rule as
-- prices: no row without a source and a date.
CREATE TABLE IF NOT EXISTS benchmarks (
    id         INTEGER PRIMARY KEY,
    entity_id  TEXT NOT NULL REFERENCES entities(id),
    suite      TEXT NOT NULL,
    metric     TEXT NOT NULL,
    value      REAL NOT NULL,
    rank       INTEGER,
    out_of     INTEGER,
    -- published: someone else measured it and published the figure. Kept
    -- apart from a provider's own claim (declared) and from what our own
    -- executions will one day observe (measured).
    basis      TEXT NOT NULL DEFAULT 'published'
               CHECK (basis IN ('declared','measured','published')),
    source_url TEXT NOT NULL,
    taken_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS benchmarks_by_entity ON benchmarks(entity_id, suite, id);
-- The suites themselves: who runs them, what the number means, which way is
-- better. A score without its suite is a number without a question.
CREATE TABLE IF NOT EXISTS suites (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    measurer        TEXT,
    url             TEXT,
    metric          TEXT,
    subject         TEXT,
    lower_is_better INTEGER NOT NULL DEFAULT 0,
    updated         TEXT,
    notes           TEXT
);
-- Source text about a subject — a model card paragraph, a company's own
-- one-liner — kept verbatim with where it came from. The About block is
-- written from these; nothing in it is remembered rather than read.
CREATE TABLE IF NOT EXISTS docs (
    id         INTEGER PRIMARY KEY,
    subject    TEXT NOT NULL,   -- entities.id or providers.id
    kind       TEXT NOT NULL,   -- description | one_liner | sells | fact
    -- '' rather than NULL: SQLite counts NULLs as distinct in UNIQUE, which
    -- would let the same sentence from the same page land twice.
    field      TEXT NOT NULL DEFAULT '',  -- for facts: founded, hq, docs_url, logo_url, …
    text       TEXT NOT NULL,
    source_url TEXT NOT NULL,
    taken_at   TEXT NOT NULL,
    UNIQUE (subject, kind, field, source_url)
);
CREATE INDEX IF NOT EXISTS docs_by_subject ON docs(subject, kind);
-- The quarantine queue (identity rule 1): a collector that meets an unknown
-- name proposes here; only a human mints the entity and binds the alias.
CREATE TABLE IF NOT EXISTS unmatched_listings (
    source     TEXT NOT NULL,
    alias      TEXT NOT NULL,
    payload    TEXT NOT NULL,
    first_seen TEXT NOT NULL,
    last_seen  TEXT NOT NULL,
    PRIMARY KEY (source, alias)
);
-- Every table a nightly job writes into is declared here, so a fresh or
-- restored index.db is fully serviceable on the first run. These once came
-- into being only when their job first ran; a job that wrote before its
-- table existed — record_run into source_runs inside supply's transaction —
-- rolled its whole delivery back while printing success.
CREATE TABLE IF NOT EXISTS source_runs (
    source     TEXT NOT NULL,
    ran_at     TEXT NOT NULL,
    fetched    INTEGER NOT NULL DEFAULT 0,
    unchanged  INTEGER NOT NULL DEFAULT 0,
    read       INTEGER NOT NULL DEFAULT 0,
    bound      INTEGER NOT NULL DEFAULT 0,
    written    INTEGER NOT NULL DEFAULT 0,
    seconds    REAL NOT NULL DEFAULT 0,
    note       TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (source, ran_at)
);
CREATE TABLE IF NOT EXISTS investments (
    fund_id    TEXT NOT NULL REFERENCES providers(id),
    company_id TEXT NOT NULL REFERENCES providers(id),
    source_url TEXT NOT NULL,
    UNIQUE (fund_id, company_id)
);
CREATE INDEX IF NOT EXISTS inv_by_fund ON investments(fund_id);
CREATE INDEX IF NOT EXISTS inv_by_company ON investments(company_id);
CREATE TABLE IF NOT EXISTS terms (
    slug   TEXT PRIMARY KEY,
    term   TEXT NOT NULL,
    kind   TEXT NOT NULL,
    short  TEXT NOT NULL,
    body   TEXT NOT NULL,
    also   TEXT NOT NULL DEFAULT '[]',
    see    TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS supply_seen (
    file    TEXT PRIMARY KEY,
    hash    TEXT NOT NULL,
    done_at TEXT NOT NULL
);
";

#[derive(Debug, Clone, Default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub url: Option<String>,
    pub kind: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Entity {
    pub id: String,
    /// model | tool | agent — the schema refuses anything else.
    pub register: String,
    pub name: String,
    pub maker: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub derived_from: Option<String>,
    pub input_kind: String,
    pub output_kind: String,
    /// Register-specific characteristics as a JSON object.
    pub attrs: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceComponent {
    pub dimension: String,
    pub micros_per_unit: i64,
    pub basis: String,
    pub source_url: String,
    pub taken_at: String,
}

#[derive(Debug, Clone)]
pub struct Metric {
    pub metric: String,
    pub value: f64,
    pub basis: String,
    pub source_url: String,
    pub taken_at: String,
}

/// One offering of an entity as the catalogue shows it: the provider, the way
/// in, and the current price components.
#[derive(Debug, Clone)]
pub struct OfferingView {
    pub offering_id: i64,
    pub provider_id: String,
    pub provider_name: String,
    pub way: String,
    pub variant: String,
    pub status: String,
    pub components: Vec<PriceComponent>,
}

pub struct Index {
    conn: Connection,
}

/// The modality vocabulary a card can render: a source may say "Synthesized
/// speech audio; the API curl example writes a WAV file", the catalogue says
/// "audio". Kept in a fixed order so two entities that take the same things
/// read the same way.
const KIND_TOKENS: [&str; 7] = ["text", "code", "image", "audio", "video", "file", "embedding"];

/// Words that name a token. Prose keywords only — a bare "file" is too common
/// in a sentence ("writes a WAV file") to mean the thing takes documents, so
/// it counts only when the whole value is already a token list. "voice" is
/// absent on purpose: a speech model takes text and a voice *name*, and prose
/// that means audio input says audio or speech.
const KIND_WORDS: [(&str, &str); 21] = [
    ("text", "text"), ("prompt", "text"), ("markdown", "text"), ("string", "text"),
    ("transcription", "text"), ("transcript", "text"), ("caption", "text"),
    ("code", "code"),
    ("image", "image"), ("picture", "image"), ("photo", "image"), ("img", "image"),
    ("audio", "audio"), ("speech", "audio"), ("sound", "audio"),
    ("music", "audio"),
    ("video", "video"), ("clip", "video"),
    ("pdf", "file"), ("document", "file"),
    ("embedding", "embedding"),
];

/// Words that name a token only in a short, already-canonical value.
const KIND_WORDS_TERSE: [(&str, &str); 4] =
    [("file", "file"), ("files", "file"), ("vector", "embedding"), ("doc", "file")];

fn has_word(hay: &str, word: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(rel) = hay[from..].find(word) {
        let at = from + rel;
        let end = at + word.len();
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        // a trailing plural or "-es" still names the same thing
        let mut tail = end;
        if bytes.get(tail) == Some(&b'e') && bytes.get(tail + 1) == Some(&b's') {
            tail += 2;
        } else if bytes.get(tail) == Some(&b's') {
            tail += 1;
        }
        let after_ok = tail >= bytes.len() || !bytes[tail].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = at + 1;
    }
    false
}

/// Read a source's words for what a thing takes or returns and answer in the
/// catalogue's vocabulary. Returns the raw value trimmed when it recognises
/// nothing short enough to keep, "text" when the prose says nothing it knows.
/// The address form of a name: lowercase, one hyphen between words, nothing
/// a URL would have to escape. Distinct from the id slug below, which keeps
/// the dots in a version number. The daily checker computes this same string
/// and refuses two things that land on one address, so a collision is caught
/// the night before anyone can reach it.
/// What a licence means for someone about to ship something. The exact SPDX
/// string is on the card; these four are the question behind it.
/// A rate as a reader would say it: two significant figures however small,
/// so a price that rounds to nothing is visibly not nothing.
/// The registrable part of a host, for deciding who is talking about whom.
/// "docs.fireworks.ai" and "https://fireworks.ai" are the same company;
/// "raw.githubusercontent.com/BerriAI/litellm" is somebody else entirely.
pub fn host_of(url: &str) -> String {
    let s = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let host = s.split('/').next().unwrap_or("").to_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join(".")
    } else {
        host
    }
}

/// Which of two reports of the same price to believe.
///
/// A seller is authoritative about its own rate: if the figure came from the
/// seller's own domain it wins, whatever else says otherwise. Everything else
/// is a third party repeating it, and among those the most recent reading is
/// the best available. Before this the winner was whichever collector ran
/// last, which is not a rule — it is an accident that changed the price on
/// the page depending on the order of a shell script.
/// Whole days from `earlier` to `later`, both `YYYY-MM-DD`, by the real
/// civil calendar — the same count SQLite's `date(x,'-45 day')` uses. It must
/// be the real calendar, not a 31-day-month approximation: the card ranks
/// prices through the SQL view (real calendar) and the browse list through
/// this function, and a month=31 shortcut made them disagree at a true
/// 45-day gap — the seller's price kept on the card, dropped from the list.
/// Negative differences clamp to zero.
fn days_between(earlier: &str, later: &str) -> i64 {
    // Days since the civil epoch (Howard Hinnant's days_from_civil), exact
    // for the proleptic Gregorian calendar and what julianday counts.
    let civil = |d: &str| -> i64 {
        let mut it = d.split('-');
        let y: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let m: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
        let day: i64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1);
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    };
    (civil(later) - civil(earlier)).max(0)
}

fn authority(source_url: &str, seller_home: &str) -> u8 {
    if seller_home.is_empty() {
        return 0;
    }
    if host_of(source_url) == host_of(seller_home) {
        2
    } else {
        0
    }
}

pub fn money(micros: i64) -> String {
    let d = micros as f64 / 1e6;
    if d <= 0.0 {
        return "$0".into();
    }
    let places = if d >= 100.0 {
        0
    } else if d >= 1.0 {
        2
    } else {
        (1.0 - d.log10().floor()).min(8.0) as usize
    };
    let s = format!("{d:.places$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    format!("${s}")
}

/// Thirteen thousand four hundred and one reads as a number; 13401 reads as
/// a serial. Anything a person is meant to take in gets its separators.
pub fn grouped(n: i64) -> String {
    let s = n.abs().to_string();
    let b: Vec<String> = s
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect();
    format!("{}{}", if n < 0 { "-" } else { "" }, b.join(","))
}

/// 1st, 2nd, 3rd — a place is spoken, not numbered.
pub fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// How a unit is said, in the two registers a page uses.
///
/// The same dimension was named in three vocabularies — "per Mtok in" in the
/// tables, "per million tokens" in the prose, "per million tokens read" in
/// the picks — and a reader comparing two blocks on one page could not tell
/// whether they were quoting the same thing. There are two registers here and
/// only two: a label, which sits beside a figure in a cell, and a phrase,
/// which is read inside a sentence. Every page takes its words from here.
pub fn unit_label(dim: &str) -> &'static str {
    match dim {
        "mtok_in" => "per Mtok in",
        "mtok_out" => "per Mtok out",
        "mtok" => "per Mtok",
        "mtok_cache_read" => "per Mtok cached",
        "mtok_cache_write" => "per Mtok cache write",
        "mtok_in_audio" => "per Mtok in, audio",
        "mtok_out_audio" => "per Mtok out, audio",
        "mtok_in_image" => "per Mtok in, image",
        "mtok_out_image" => "per Mtok out, image",
        "mtok_cache_read_audio" => "per Mtok cached, audio",
        "mtok_cache_read_image" => "per Mtok cached, image",
        "mtok_out_reasoning" => "per Mtok out, reasoning",
        "image" => "per image",
        "image_in" => "per image in",
        "second" => "per second",
        "second_in" => "per second in",
        "second_out" => "per second out",
        "minute" => "per minute",
        "call" => "per call",
        "character" => "per character",
        "page" => "per page",
        "result" => "per result",
        "month" => "per month",
        _ => "",
    }
}

/// The same unit as a sentence says it: "$4 per million tokens", "$20 a month".
pub fn unit_phrase(dim: &str) -> &'static str {
    match dim {
        "mtok_in" | "mtok" | "mtok_out" => "per million tokens",
        "mtok_cache_read" => "per million tokens cached",
        "mtok_in_audio" | "mtok_out_audio" => "per million audio tokens",
        "month" => "a month",
        "image" | "image_in" => "an image",
        "second" | "second_in" | "second_out" => "a second",
        "minute" => "a minute",
        "character" => "a character",
        "page" => "a page",
        "call" => "a call",
        "result" => "a result",
        _ => "",
    }
}

/// The size bands a reader chooses between, and what each one means for the
/// machine they have. Weights at four-bit quantisation cost about 0.65 GB per
/// billion, so the band a model falls in is the first thing that decides
/// whether it can run anywhere near you.
pub const SIZE_BANDS: &[(&str, &str, f64, f64, &str)] = &[
    ("nano", "Nano", 0.0, 3.0, "under three billion — a phone, a browser, a Raspberry Pi"),
    ("xs", "Extra small", 3.0, 10.0, "three to ten billion — any laptop with 8 GB to spare"),
    ("s", "Small", 10.0, 30.0, "ten to thirty billion — a good laptop, or one consumer card"),
    ("m", "Medium", 30.0, 100.0, "thirty to a hundred billion — a workstation, or a 64 GB Mac"),
    ("l", "Large", 100.0, 300.0, "a hundred to three hundred billion — a server card, or several"),
    ("xl", "Extra large", 300.0, 1000.0,
     "three hundred billion to a trillion — a machine you rent by the hour"),
    ("frontier", "Frontier", 1000.0, f64::INFINITY,
     "a trillion parameters and up — nobody runs these but their makers and the clouds"),
];

pub fn size_band(billions: f64) -> Option<&'static (&'static str, &'static str, f64, f64, &'static str)> {
    SIZE_BANDS.iter().find(|(_, _, lo, hi, _)| billions >= *lo && billions < *hi)
}

pub const LICENCE_FAMILIES: &[(&str, &str, &str)] = &[
    ("open", "open weights you may use commercially",
     "json_extract(attrs,'$.license') IN ('apache-2.0','mit','cc-by-4.0','bsd-3-clause','openmdw-1.1')"),
    ("open-with-conditions", "open weights with conditions attached",
     "json_extract(attrs,'$.license') IS NOT NULL \
      AND json_extract(attrs,'$.license') NOT IN ('apache-2.0','mit','cc-by-4.0','bsd-3-clause','openmdw-1.1','proprietary') \
      AND json_extract(attrs,'$.license') NOT LIKE 'cc-by-nc%'"),
    ("noncommercial", "published for research only, not for selling",
     "json_extract(attrs,'$.license') LIKE 'cc-by-nc%'"),
    ("proprietary", "closed weights, bought through an API",
     "json_extract(attrs,'$.license') = 'proprietary'"),
];

/// What fits in a device's memory. Weights at four-bit quantisation cost
/// about 0.65 GB per billion parameters; a machine can lend roughly seven
/// tenths of its memory to the model and needs a gigabyte back for the
/// context and the runtime. The parameter count is the total, never the
/// active one — a mixture of experts computes with a few and holds them all.
pub const MEMORY_BANDS: &[(&str, f64, &str)] = &[
    ("8gb", 8.0, "a phone, a base iPad, an Air"),
    ("16gb", 16.0, "the common laptop"),
    ("24gb", 24.0, "a 24 GB card, or a Mac with 24"),
    ("32gb", 32.0, "a well-specified laptop"),
    ("36gb", 36.0, "a MacBook Pro with 36"),
    ("64gb", 64.0, "a workstation"),
    ("96gb", 96.0, "a MacBook Pro with 96"),
    ("128gb", 128.0, "a large Mac or a server card"),
    ("256gb", 256.0, "a Mac Studio with 256"),
];

/// The weights a device of this size can hold, in billions of parameters.
pub fn fits_billions(device_gb: f64) -> f64 {
    ((device_gb * 0.7) - 1.0).max(0.0) / 0.65
}

pub fn address_slug(s: &str) -> String {
    let expanded = s.to_lowercase().replace('+', " plus ").replace('&', " and ");
    let mut out = String::new();
    for ch in expanded.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn normalise_kind(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let lower = raw.to_lowercase();
    let terse = raw.len() <= 40;
    let mut found: Vec<&str> = Vec::new();
    for (word, token) in KIND_WORDS
        .iter()
        .chain(KIND_WORDS_TERSE.iter().filter(|_| terse))
    {
        if has_word(&lower, word) && !found.contains(token) {
            found.push(token);
        }
    }
    if found.is_empty() {
        return if terse { lower } else { "text".into() };
    }
    KIND_TOKENS
        .iter()
        .filter(|t| found.contains(t))
        .copied()
        .collect::<Vec<_>>()
        .join(" + ")
}

/// An id the catalogue can live with: lowercase, and every run of anything
/// else becomes one hyphen. Dots stay — a version is part of the name.
fn slug(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches(['-', '.']).to_string()
}

/// What a listing says it takes and returns. A source that names its
/// modalities is believed; one that only names a task type is read through
/// the task; one that says neither gets text, the catalogue's plainest guess.
fn kinds_from_payload(p: &Value) -> (String, String) {
    let modalities = |field: &str| -> Option<String> {
        let arr = p[field].as_array()?;
        let joined = arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        (!joined.is_empty()).then(|| normalise_kind(&joined))
    };
    if let (Some(i), Some(o)) = (modalities("input_modalities"), modalities("output_modalities")) {
        return (i, o);
    }
    let by_task: &[(&str, &str, &str)] = &[
        ("text-to-image", "text", "image"),
        ("image-to-image", "image", "image"),
        ("text-to-video", "text", "video"),
        ("image-to-video", "image", "video"),
        ("text-to-speech", "text", "audio"),
        ("automatic-speech-recognition", "audio", "text"),
        ("embeddings", "text", "embedding"),
        ("text-to-embedding", "text", "embedding"),
        ("zero-shot-image-classification", "image", "text"),
        ("object-detection", "image", "text"),
        ("text-classification", "text", "text"),
    ];
    let task = p["kind"].as_str().unwrap_or("");
    for (name, i, o) in by_task {
        if task == *name {
            return (i.to_string(), o.to_string());
        }
    }
    ("text".into(), "text".into())
}

impl Index {
    /// Open (or create) the catalogue at `path` through the one SQLite door.
    /// The open connection, for the parts of the crate that read the
    /// catalogue's own tables directly — the resolver indexes every name and
    /// alias once at start-up and there is no smaller door for that.
    pub fn conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    pub fn open(path: &str) -> Result<Self> {
        let conn = common::db::open(path)?;
        prepare(&conn)?;
        Ok(Self { conn })
    }
}

/// Bring any freshly opened connection up to the full schema: every table a
/// job writes, the foreign-key pragma, the allowance column on an old db,
/// and the current-prices view. Every binary that opens the catalogue must
/// call this — a `repair` or `exportkb` run over a restored or rebuilt db
/// used to fail with "no such table: current_prices", and `record_run` wrote
/// into a source_runs that a fresh db did not yet have, rolling a whole
/// delivery back while printing success.
pub fn prepare(conn: &Connection) -> Result<()> {
    {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        // A free allowance in words cannot be added up, and a reader wanting
        // to know what a day of free access is worth needs it added up. The
        // prose stays in `limits`; the figures behind it go here, as JSON, so
        // the page can total them without parsing English.
        let has = conn
            .prepare("SELECT 1 FROM pragma_table_info('offerings') WHERE name='allowance'")?
            .exists([])?;
        if !has {
            conn.execute_batch("ALTER TABLE offerings ADD COLUMN allowance TEXT")?;
        }
        // The current price of an offering, one row per dimension. `prices`
        // is append-only history, and a bare MIN over it advertises rates
        // that were withdrawn — the under-a-dollar list held prices nobody
        // charged. Where two sources still disagree, the seller's own page
        // outranks a third-party catalogue, which is the rule the checks
        // suite already states in words. Recreated on every open so the
        // definition in the file is the definition in the database. A page
        // request opens the catalogue too, so the rewrite happens only when
        // the stored definition differs — open stays read-only in the steady
        // state.
        let want = "current-prices-v3";
        let have: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name='current_prices'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if !have.map(|h| h.contains(want)).unwrap_or(false) {
        // The seller-domain preference decays: a row from the seller's own
        // page wins only while it is within 45 days of the freshest word on
        // the same offering+dimension. Past that gap its domain bonus is
        // withdrawn and the newest figure wins, so a broken seller-page
        // collector can no longer pin a rate nobody charges. The 45-day window
        // matches find_index's days_between rule so card and list agree.
        conn.execute_batch(
            "DROP VIEW IF EXISTS current_prices;
             CREATE VIEW current_prices AS\n             -- current-prices-v3
             SELECT id, offering_id, dimension, micros_per_unit, basis,
                    source_url, taken_at
               FROM (SELECT id, offering_id, dimension, micros_per_unit, basis,
                            source_url, taken_at,
                            ROW_NUMBER() OVER (
                              PARTITION BY offering_id, dimension
                              -- the seller's own page, but only while fresh
                              ORDER BY (on_seller_page
                                        AND taken_at >= date(newest, '-45 day')) DESC,
                                       taken_at DESC,
                                       id DESC) AS rn
                       FROM (SELECT p.*,
                                    (pr.url <> '' AND p.source_url LIKE
                                       '%' || substr(
                                               replace(replace(pr.url,'https://',''),
                                                       'http://',''),
                                               1,
                                               instr(replace(replace(pr.url,'https://',''),
                                                             'http://','')
                                                     || '/', '/') - 1)
                                       || '%') AS on_seller_page,
                                    MAX(p.taken_at) OVER (
                                      PARTITION BY p.offering_id, p.dimension) AS newest
                               FROM prices p
                               JOIN offerings o ON o.id = p.offering_id
                               JOIN providers pr ON pr.id = o.provider_id))
              WHERE rn = 1;
",
        )?;
        }
        Ok(())
    }
}

impl Index {
    pub fn upsert_provider(&self, p: &Provider) -> Result<()> {
        self.conn.execute(
            "INSERT INTO providers (id, name, url, kind, notes) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, url=excluded.url,
                 kind=excluded.kind, notes=excluded.notes",
            params![p.id, p.name, p.url, p.kind, p.notes],
        )?;
        Ok(())
    }

    /// Entities are created deliberately, never by a collector (identity rule 1).
    pub fn insert_entity(&self, e: &Entity) -> Result<()> {
        self.conn.execute(
            "INSERT INTO entities (id, register, name, maker, family, version,
                                   derived_from, input_kind, output_kind, attrs)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                e.id, e.register, e.name, e.maker, e.family, e.version,
                e.derived_from, normalise_kind(&e.input_kind), normalise_kind(&e.output_kind),
                e.attrs
            ],
        )?;
        Ok(())
    }

    /// What the source says the thing is: the modalities it takes and returns
    /// and the context it holds. A field the source does not state is left
    /// alone rather than blanked. Returns true when something changed.
    pub fn set_entity_facts(
        &self,
        entity_id: &str,
        input_kind: Option<&str>,
        output_kind: Option<&str>,
        context: Option<i64>,
    ) -> Result<bool> {
        let Some((cur_in, cur_out, attrs)): Option<(String, String, String)> = self
            .conn
            .query_row(
                "SELECT input_kind, output_kind, attrs FROM entities WHERE id=?1",
                params![entity_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        let mut attrs: serde_json::Map<String, Value> =
            serde_json::from_str(&attrs).unwrap_or_default();
        let new_in = input_kind.map(normalise_kind).unwrap_or_else(|| cur_in.clone());
        let new_out = output_kind.map(normalise_kind).unwrap_or_else(|| cur_out.clone());
        let ctx_changed = match context {
            Some(c) => attrs.insert("context".into(), json!(c)) != Some(json!(c)),
            None => false,
        };
        if new_in == cur_in && new_out == cur_out && !ctx_changed {
            return Ok(false);
        }
        self.conn.execute(
            "UPDATE entities SET input_kind=?2, output_kind=?3, attrs=?4 WHERE id=?1",
            params![entity_id, new_in, new_out, Value::Object(attrs).to_string()],
        )?;
        Ok(true)
    }

    /// Bind what `source` calls an entity to the entity itself (identity rule 2).
    pub fn bind_alias(&self, source: &str, alias: &str, entity_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO aliases (source, alias, entity_id) VALUES (?1,?2,?3)
             ON CONFLICT(source, alias) DO UPDATE SET entity_id=excluded.entity_id",
            params![source, alias, entity_id],
        )?;
        Ok(())
    }

    pub fn resolve(&self, source: &str, alias: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT entity_id FROM aliases WHERE source=?1 AND alias=?2",
                params![source, alias],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// One row per (entity, provider, way, variant); a repeat sighting moves
    /// `last_seen` and revives the status.
    pub fn upsert_offering(
        &self,
        entity_id: &str,
        provider_id: &str,
        way: &str,
        variant: &str,
        seen_at: &str,
    ) -> Result<i64> {
        self.conn.query_row(
            "INSERT INTO offerings (entity_id, provider_id, way, variant, first_seen, last_seen)
             VALUES (?1,?2,?3,?4,?5,?5)
             ON CONFLICT(entity_id, provider_id, way, variant)
             DO UPDATE SET last_seen=excluded.last_seen, status='live'
             RETURNING id",
            params![entity_id, provider_id, way, variant, seen_at],
            |r| r.get(0),
        )
        .map_err(Into::into)
    }

    /// Append one declared price component. Provenance is part of the row.
    pub fn add_price(
        &self,
        offering_id: i64,
        dimension: &str,
        micros_per_unit: i64,
        source_url: &str,
        taken_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO prices (offering_id, dimension, micros_per_unit, source_url, taken_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![offering_id, dimension, micros_per_unit, source_url, taken_at],
        )?;
        Ok(())
    }

    /// The current price: the latest component per dimension. History stays.
    pub fn current_price(&self, offering_id: i64) -> Result<Vec<PriceComponent>> {
        // Through the view, so the card and the lists answer with one voice:
        // the seller's own page outranks a third-party catalogue, newest
        // otherwise.
        let mut stmt = self.conn.prepare(
            "SELECT dimension, micros_per_unit, basis, source_url, taken_at
             FROM current_prices WHERE offering_id = ?1
             ORDER BY dimension",
        )?;
        let rows = stmt
            .query_map(params![offering_id], |r| {
                Ok(PriceComponent {
                    dimension: r.get(0)?,
                    micros_per_unit: r.get(1)?,
                    basis: r.get(2)?,
                    source_url: r.get(3)?,
                    taken_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Append the component only when it changes the current price: a daily
    /// collector run over stable prices writes nothing, so history stays a
    /// record of movement. Returns true when a row was appended.
    pub fn add_price_if_changed(
        &self,
        offering_id: i64,
        dimension: &str,
        micros_per_unit: i64,
        source_url: &str,
        taken_at: &str,
    ) -> Result<bool> {
        // Compared against this source's own last figure, not against
        // whichever source wrote most recently. Two sources that disagree
        // used to take turns "changing" the price every night — each saw the
        // other's figure as current and re-asserted its own — and the history
        // grew a row a night recording no movement at all.
        let same: Option<i64> = self
            .conn
            .query_row(
                "SELECT micros_per_unit FROM prices                   WHERE offering_id=?1 AND dimension=?2 AND source_url=?3                   ORDER BY id DESC LIMIT 1",
                params![offering_id, dimension, source_url],
                |r| r.get(0),
            )
            .optional()?;
        if same == Some(micros_per_unit) {
            return Ok(false);
        }
        self.add_price(offering_id, dimension, micros_per_unit, source_url, taken_at)?;
        Ok(true)
    }

    /// Quarantine a listing the collector could not bind (identity rule 1).
    /// A repeat sighting refreshes the payload and `last_seen`.
    pub fn upsert_unmatched(
        &self,
        source: &str,
        alias: &str,
        payload: &str,
        seen_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO unmatched_listings (source, alias, payload, first_seen, last_seen)
             VALUES (?1,?2,?3,?4,?4)
             ON CONFLICT(source, alias)
             DO UPDATE SET payload=excluded.payload, last_seen=excluded.last_seen",
            params![source, alias, payload, seen_at],
        )?;
        Ok(())
    }

    /// The quarantine queue: (source, alias, payload, last_seen).
    pub fn unmatched(&self) -> Result<Vec<(String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT source, alias, payload, last_seen FROM unmatched_listings
             ORDER BY source, alias",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Take a source's whole quarantine queue into the catalogue: mint what
    /// the catalogue does not carry, bind what it does, and open a provider
    /// row for a maker it has never heard of. This is identity rule 1 held to
    /// deliberately — a person runs `absorb`, the collector never does — but
    /// at the scale a market of thousands of listings actually has. Every
    /// figure still comes from the source; only the naming is automatic, and
    /// a wrong split is repaired with `merge`.
    pub fn absorb(&self, source: &str, limit: usize) -> Result<(usize, usize, usize, usize)> {
        let rows: Vec<(String, String)> = self
            .conn
            .prepare("SELECT alias, payload FROM unmatched_listings WHERE source=?1 ORDER BY alias")?
            .query_map(params![source], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let (mut minted, mut bound, mut opened, mut skipped) = (0, 0, 0, 0);
        for (alias, payload) in rows.into_iter().take(if limit == 0 { usize::MAX } else { limit }) {
            let p: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
            let last = alias.rsplit('/').next().unwrap_or(&alias);
            let id = format!("ent_{}", slug(last));
            if id == "ent_" {
                skipped += 1;
                continue;
            }
            let exists: bool = self
                .conn
                .query_row("SELECT 1 FROM entities WHERE id=?1", params![id], |_| Ok(true))
                .optional()?
                .unwrap_or(false);
            if !exists {
                // The maker is whoever the source names before the slash; a
                // bare name has no maker, and the catalogue says so rather
                // than crediting the seller with the weights.
                let maker = match alias.split_once('/') {
                    Some((org, _)) if !org.is_empty() => {
                        let (pid, fresh) = self.provider_for_org(org)?;
                        if fresh {
                            opened += 1;
                        }
                        Some(pid)
                    }
                    _ => None,
                };
                let (input_kind, output_kind) = kinds_from_payload(&p);
                let mut attrs = serde_json::Map::new();
                if let Some(ctx) = p["context_length"].as_i64().filter(|c| *c > 0) {
                    attrs.insert("context".into(), json!(ctx));
                }
                self.insert_entity(&Entity {
                    id: id.clone(),
                    register: "model".into(),
                    name: p["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(last).to_string(),
                    maker,
                    family: None,
                    version: None,
                    derived_from: None,
                    input_kind,
                    output_kind,
                    attrs: Value::Object(attrs).to_string(),
                })?;
                minted += 1;
            }
            self.bind_alias(source, &alias, &id)?;
            self.remove_unmatched(source, &alias)?;
            bound += 1;
        }
        Ok((minted, bound, opened, skipped))
    }

    /// The provider row for an organisation a listing names. Known makers
    /// keep the id the catalogue already uses; an unknown one gets a row of
    /// its own, empty but real, for a later crawl to describe.
    fn provider_for_org(&self, org: &str) -> Result<(String, bool)> {
        let known: &[(&str, &str)] = &[
            ("anthropic", "prov_anthropic"), ("openai", "prov_openai"),
            ("meta-llama", "prov_meta"), ("meta", "prov_meta"),
            ("google", "prov_google"), ("qwen", "prov_qwen"), ("alibaba-nlp", "prov_qwen"),
            ("deepseek-ai", "prov_deepseek"), ("deepseek", "prov_deepseek"),
            ("zai-org", "prov_zhipu"), ("z-ai", "prov_zhipu"), ("thudm", "prov_zhipu"),
            ("moonshotai", "prov_moonshot"), ("mistralai", "prov_mistral"),
            ("nvidia", "prov_nvidia"), ("microsoft", "prov_microsoft"),
            ("black-forest-labs", "prov_bfl"), ("minimaxai", "prov_minimax"),
            ("bytedance", "prov_bytedance"), ("bytedance-seed", "prov_bytedance"),
            ("stabilityai", "prov_stability"), ("ibm-granite", "prov_ibm"),
            ("cohere", "prov_cohere"), ("cohereforai", "prov_cohere"),
            ("nousresearch", "prov_nous"), ("allenai", "prov_allenai"),
            ("thinkingmachines", "prov_thinking-machines"), ("xai", "prov_xai"),
            ("x-ai", "prov_xai"), ("canopylabs", "prov_canopy"), ("hexgrad", "prov_hexgrad"),
            ("intfloat", "prov_intfloat"), ("perplexity", "prov_perplexity"),
            ("writer", "prov_writer"), ("baidu", "prov_baidu"), ("tencent", "prov_tencent"),
            ("upstage", "prov_upstage"), ("stepfun-ai", "prov_stepfun"),
            ("inclusionai", "prov_inclusionai"), ("meituan-longcat", "prov_meituan"),
            ("xiaomimimo", "prov_xiaomi"), ("kwaipilot", "prov_kuaishou"),
        ];
        let key = org.to_lowercase();
        if let Some((_, pid)) = known.iter().find(|(name, _)| *name == key) {
            return Ok((pid.to_string(), false));
        }
        let pid = format!("prov_{}", slug(org));
        let exists: bool = self
            .conn
            .query_row("SELECT 1 FROM providers WHERE id=?1", params![pid], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !exists {
            self.upsert_provider(&Provider {
                id: pid.clone(),
                name: org.to_string(),
                url: None,
                kind: Some("vendor".into()),
                notes: None,
            })?;
            return Ok((pid, true));
        }
        Ok((pid, false))
    }

    /// Remove a quarantined listing once its alias is bound.
    pub fn remove_unmatched(&self, source: &str, alias: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM unmatched_listings WHERE source=?1 AND alias=?2",
            params![source, alias],
        )?;
        Ok(n > 0)
    }

    /// Drop offerings that carry no price and no metric. A `local` offering
    /// legitimately has neither — the weights run on your own hardware — so
    /// only the priced ways are swept. Returns how many were dropped.
    pub fn drop_empty_offerings(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM offerings WHERE way <> 'local'
               AND id NOT IN (SELECT offering_id FROM prices)
               AND id NOT IN (SELECT offering_id FROM metrics)",
            [],
        )?)
    }

    /// Drop this source's quarantined listings outside `seen` — the aliases
    /// the run just read from that source. The queue then means one thing:
    /// what the source offers now and the catalogue cannot yet name. Keyed on
    /// the run's own aliases, not on a date, because two runs in one day may
    /// read the same source through different parsing rules.
    pub fn prune_unmatched(&self, source: &str, seen: &[String]) -> Result<usize> {
        // An empty read is the collector falling silent, not the source
        // emptying: pruning on it would wipe the whole quarantine queue for a
        // source that simply failed to fetch this run. Nothing seen, nothing
        // pruned.
        if seen.is_empty() {
            return Ok(0);
        }
        let mut sql = String::from("DELETE FROM unmatched_listings WHERE source = ?1");
        if !seen.is_empty() {
            sql.push_str(" AND alias NOT IN (");
            for i in 0..seen.len() {
                if i > 0 {
                    sql.push(',');
                }
                sql.push_str(&format!("?{}", i + 2));
            }
            sql.push(')');
        }
        let mut values: Vec<&dyn rusqlite::ToSql> = vec![&source];
        values.extend(seen.iter().map(|s| s as &dyn rusqlite::ToSql));
        Ok(self.conn.execute(&sql, values.as_slice())?)
    }

    /// Append one declared speed/quality figure.
    pub fn add_metric(
        &self,
        offering_id: i64,
        metric: &str,
        value: f64,
        source_url: &str,
        taken_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO metrics (offering_id, metric, value, source_url, taken_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![offering_id, metric, value, source_url, taken_at],
        )?;
        Ok(())
    }

    /// Append the figure only when it changes the current value of that
    /// metric — same discipline as prices. Returns true when appended.
    pub fn add_metric_if_changed(
        &self,
        offering_id: i64,
        metric: &str,
        value: f64,
        source_url: &str,
        taken_at: &str,
    ) -> Result<bool> {
        let current = self.current_metrics(offering_id)?;
        if current.iter().any(|m| m.metric == metric && m.value == value) {
            return Ok(false);
        }
        self.add_metric(offering_id, metric, value, source_url, taken_at)?;
        Ok(true)
    }

    /// The current metrics: the latest figure per metric name.
    pub fn current_metrics(&self, offering_id: i64) -> Result<Vec<Metric>> {
        let mut stmt = self.conn.prepare(
            "SELECT metric, value, basis, source_url, taken_at
             FROM metrics m
             WHERE offering_id = ?1
               AND id = (SELECT MAX(id) FROM metrics
                         WHERE offering_id = m.offering_id AND metric = m.metric)
             ORDER BY metric",
        )?;
        let rows = stmt
            .query_map(params![offering_id], |r| {
                Ok(Metric {
                    metric: r.get(0)?,
                    value: r.get(1)?,
                    basis: r.get(2)?,
                    source_url: r.get(3)?,
                    taken_at: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Register a suite: who measures it, what its number means.
    pub fn upsert_suite(
        &self,
        id: &str,
        name: &str,
        measurer: Option<&str>,
        url: Option<&str>,
        metric: Option<&str>,
        subject: Option<&str>,
        lower_is_better: bool,
        updated: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO suites (id, name, measurer, url, metric, subject, lower_is_better, updated)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, measurer=excluded.measurer,
                 url=excluded.url, metric=excluded.metric, subject=excluded.subject,
                 lower_is_better=excluded.lower_is_better, updated=excluded.updated",
            params![id, name, measurer, url, metric, subject, lower_is_better as i64, updated],
        )?;
        Ok(())
    }

    /// Append a published score, but only when it moves: a leaderboard read
    /// twice in a week should leave one row, not two.
    #[allow(clippy::too_many_arguments)]
    pub fn add_benchmark_if_changed(
        &self,
        entity_id: &str,
        suite: &str,
        metric: &str,
        value: f64,
        rank: Option<i64>,
        out_of: Option<i64>,
        source_url: &str,
        taken_at: &str,
    ) -> Result<bool> {
        // The size of the field is part of the standing: third of five and
        // third of five hundred are not the same result, so learning the
        // total counts as news even when value and rank have not moved.
        let current: Option<(f64, Option<i64>, Option<i64>)> = self
            .conn
            .query_row(
                "SELECT value, rank, out_of FROM benchmarks b
                 WHERE entity_id=?1 AND suite=?2 AND metric=?3
                   AND id=(SELECT MAX(id) FROM benchmarks
                           WHERE entity_id=b.entity_id AND suite=b.suite AND metric=b.metric)",
                params![entity_id, suite, metric],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if current == Some((value, rank, out_of)) {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO benchmarks (entity_id, suite, metric, value, rank, out_of, source_url, taken_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![entity_id, suite, metric, value, rank, out_of, source_url, taken_at],
        )?;
        Ok(true)
    }

    /// The current standing of one entity, one row per suite+metric: the
    /// newest reading, and within that reading the best rank. Newest first so
    /// a model that slipped shows its current rank, not a stale better one;
    /// best-rank-within-the-reading so a board that lists a model in several
    /// configurations at once (eight terminal-bench rows one night) shows the
    /// best, not whichever was inserted last.
    pub fn benchmarks_of(&self, entity_id: &str) -> Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT b.suite, s.name, s.measurer, s.url, s.lower_is_better,
                    b.metric, b.value, b.rank, b.out_of, b.basis, b.source_url, b.taken_at
             FROM benchmarks b LEFT JOIN suites s ON s.id = b.suite
             WHERE b.entity_id = ?1
               AND b.id = (SELECT id FROM benchmarks
                           WHERE entity_id=b.entity_id AND suite=b.suite AND metric=b.metric
                           ORDER BY taken_at DESC, (rank IS NULL), rank, id DESC LIMIT 1)
             ORDER BY (b.rank IS NULL), b.rank, s.name, b.metric, b.id",
        )?;
        let rows = stmt
            .query_map(params![entity_id], |r| {
                Ok(json!({
                    "suite": r.get::<_, String>(0)?,
                    "suite_name": r.get::<_, Option<String>>(1)?,
                    "measurer": r.get::<_, Option<String>>(2)?,
                    "suite_url": r.get::<_, Option<String>>(3)?,
                    "lower_is_better": r.get::<_, i64>(4).unwrap_or(0) == 1,
                    "metric": r.get::<_, String>(5)?,
                    "value": r.get::<_, f64>(6)?,
                    "rank": r.get::<_, Option<i64>>(7)?,
                    "out_of": r.get::<_, Option<i64>>(8)?,
                    "basis": r.get::<_, String>(9)?,
                    "source": r.get::<_, String>(10)?,
                    "taken_at": r.get::<_, String>(11)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Keep a piece of source text about a subject, with where it came from.
    pub fn upsert_doc(
        &self,
        subject: &str,
        kind: &str,
        field: Option<&str>,
        text: &str,
        source_url: &str,
        taken_at: &str,
    ) -> Result<()> {
        let field = field.unwrap_or("");
        self.conn.execute(
            "INSERT INTO docs (subject, kind, field, text, source_url, taken_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(subject, kind, field, source_url)
             DO UPDATE SET text=excluded.text, taken_at=excluded.taken_at",
            params![subject, kind, field, text, source_url, taken_at],
        )?;
        Ok(())
    }

    /// Everything read about a subject, newest first.
    pub fn docs_of(&self, subject: &str) -> Result<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            // Newest first within a kind, and the address breaks a tie, so a
            // subject described twice shows the same one on every render
            // rather than whichever row the table handed back first.
            "SELECT kind, field, text, source_url, taken_at FROM docs
             WHERE subject = ?1 ORDER BY kind, field, taken_at DESC, source_url",
        )?;
        let rows = stmt
            .query_map(params![subject], |r| {
                Ok(json!({
                    "kind": r.get::<_, String>(0)?,
                    "field": r.get::<_, String>(1)?,
                    "text": r.get::<_, String>(2)?,
                    "source": r.get::<_, String>(3)?,
                    "taken_at": r.get::<_, String>(4)?,
                }))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Fold one entity into another: the same weights minted twice under two
    /// sources' names. Everything hanging off the loser moves to the
    /// survivor; an offering that would collide with one the survivor already
    /// has takes `variant_hint` instead of overwriting it, because two
    /// deployments of one model at one provider are two lanes, not one.
    /// Returns (offerings moved, aliases moved, standings moved, texts moved).
    pub fn merge_entity(
        &self,
        loser: &str,
        survivor: &str,
        variant_hint: &str,
    ) -> Result<(usize, usize, usize, usize)> {
        if loser == survivor {
            anyhow::bail!("an entity cannot be merged into itself");
        }
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM entities WHERE id IN (?1, ?2)",
            params![loser, survivor],
            |r| r.get(0),
        )?;
        if exists != 2 {
            anyhow::bail!("both {loser} and {survivor} must exist");
        }
        let mut offerings = 0;
        let moving: Vec<(i64, String, String, String)> = self
            .conn
            .prepare("SELECT id, provider_id, way, variant FROM offerings WHERE entity_id=?1")?
            .query_map(params![loser], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, provider, way, variant) in moving {
            let taken: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM offerings
                 WHERE entity_id=?1 AND provider_id=?2 AND way=?3 AND variant=?4",
                params![survivor, provider, way, variant],
                |r| r.get(0),
            )?;
            let variant = if taken > 0 {
                format!("{variant} {variant_hint}").trim().to_string()
            } else {
                variant
            };
            self.conn.execute(
                "UPDATE offerings SET entity_id=?2, variant=?3 WHERE id=?1",
                params![id, survivor, variant],
            )?;
            offerings += 1;
        }
        let aliases = self.conn.execute(
            "UPDATE OR REPLACE aliases SET entity_id=?2 WHERE entity_id=?1",
            params![loser, survivor],
        )?;
        let standings = self.conn.execute(
            "UPDATE benchmarks SET entity_id=?2 WHERE entity_id=?1",
            params![loser, survivor],
        )?;
        let texts = self.conn.execute(
            "UPDATE OR REPLACE docs SET subject=?2 WHERE subject=?1",
            params![loser, survivor],
        )?;
        self.conn.execute("UPDATE entities SET derived_from=?2 WHERE derived_from=?1",
            params![loser, survivor])?;
        self.conn.execute("DELETE FROM entities WHERE id=?1", params![loser])?;
        Ok((offerings, aliases, standings, texts))
    }

    /// Browse: the entities of one register.
    /// Re-read every modality column in the catalogue's own vocabulary. Rows
    /// written before the normaliser, or by hand, are brought into line here.
    /// Returns how many entities changed.
    pub fn tidy_kinds(&self) -> Result<usize> {
        let rows: Vec<(String, String, String)> = self
            .conn
            .prepare("SELECT id, input_kind, output_kind FROM entities")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut changed = 0;
        for (id, input, output) in rows {
            let (new_in, new_out) = (normalise_kind(&input), normalise_kind(&output));
            if new_in == input && new_out == output {
                continue;
            }
            self.conn.execute(
                "UPDATE entities SET input_kind=?2, output_kind=?3 WHERE id=?1",
                params![id, new_in, new_out],
            )?;
            changed += 1;
        }
        Ok(changed)
    }

    pub fn entities(&self, register: &str) -> Result<Vec<Entity>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, register, name, maker, family, version, derived_from,
                    input_kind, output_kind, attrs
             FROM entities WHERE register = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map(params![register], |r| {
                Ok(Entity {
                    id: r.get(0)?,
                    register: r.get(1)?,
                    name: r.get(2)?,
                    maker: r.get(3)?,
                    family: r.get(4)?,
                    version: r.get(5)?,
                    derived_from: r.get(6)?,
                    input_kind: r.get(7)?,
                    output_kind: r.get(8)?,
                    attrs: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// One entity's provider list — the maker sits in it as an ordinary row —
    /// with each offering's current price components.
    pub fn offerings_of(&self, entity_id: &str) -> Result<Vec<OfferingView>> {
        let mut stmt = self.conn.prepare(
            "SELECT o.id, o.provider_id, pr.name, o.way, o.variant, o.status
             FROM offerings o JOIN providers pr ON pr.id = o.provider_id
             WHERE o.entity_id = ?1
             ORDER BY pr.name, o.way, o.variant",
        )?;
        let mut views = stmt
            .query_map(params![entity_id], |r| {
                Ok(OfferingView {
                    offering_id: r.get(0)?,
                    provider_id: r.get(1)?,
                    provider_name: r.get(2)?,
                    way: r.get(3)?,
                    variant: r.get(4)?,
                    status: r.get(5)?,
                    components: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for v in &mut views {
            v.components = self.current_price(v.offering_id)?;
        }
        Ok(views)
    }

    /// Every provider in the catalogue.
    pub fn providers(&self) -> Result<Vec<Provider>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, url, kind, notes FROM providers ORDER BY name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Provider {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    url: r.get(2)?,
                    kind: r.get(3)?,
                    notes: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// What the sources call this entity: (source, alias) pairs.
    pub fn aliases_of(&self, entity_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source, alias FROM aliases WHERE entity_id = ?1 ORDER BY source, alias")?;
        let rows = stmt
            .query_map(params![entity_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The whole catalogue as one JSON value: entities with their offerings,
    /// current price components, current metrics and aliases. Feeds the
    /// browser page and the export binary.
    /// One product as the pages and the JSON twin want it. export_json builds
    /// the same shape for every row at once; a page needs exactly one, and
    /// walking the whole catalogue to find it cost two seconds a request.
    pub fn entity_json(&self, id: &str) -> Result<Option<Value>> {
        let mut q = self.conn.prepare(
            "SELECT id, register, name, maker, family, version, derived_from, \
                    input_kind, output_kind, attrs FROM entities WHERE id=?1",
        )?;
        let e = q
            .query_row(params![id], |r| {
                Ok(Entity {
                    id: r.get(0)?,
                    register: r.get(1)?,
                    name: r.get(2)?,
                    maker: r.get(3)?,
                    family: r.get(4)?,
                    version: r.get(5)?,
                    derived_from: r.get(6)?,
                    input_kind: r.get(7)?,
                    output_kind: r.get(8)?,
                    attrs: r.get(9)?,
                })
            })
            .optional()?;
        let Some(e) = e else { return Ok(None) };
        Ok(Some(self.one_entity_json(&e)?))
    }

    fn one_entity_json(&self, e: &Entity) -> Result<Value> {
        let offerings: Vec<Value> = self
            .offerings_of(&e.id)?
            .into_iter()
            .map(|o| {
                let metrics: Vec<Value> = self
                    .current_metrics(o.offering_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| {
                        json!({"metric": m.metric, "value": m.value, "basis": m.basis,
                               "source": m.source_url, "taken_at": m.taken_at})
                    })
                    .collect();
                let components: Vec<Value> = o
                    .components
                    .iter()
                    .map(|c| {
                        json!({"dimension": c.dimension, "micros_per_unit": c.micros_per_unit,
                               "basis": c.basis, "source": c.source_url, "taken_at": c.taken_at})
                    })
                    .collect();
                json!({"provider_id": o.provider_id, "provider": o.provider_name,
                       "way": o.way, "variant": o.variant, "status": o.status,
                       "components": components, "metrics": metrics})
            })
            .collect();
        let aliases: Vec<Value> = self
            .aliases_of(&e.id)?
            .into_iter()
            .map(|(source, alias)| json!({"source": source, "alias": alias}))
            .collect();
        Ok(json!({
            "benchmarks": self.benchmarks_of(&e.id)?,
            "docs": self.docs_of(&e.id)?,
            "id": e.id, "register": e.register, "name": e.name, "maker": e.maker,
            "family": e.family, "version": e.version, "derived_from": e.derived_from,
            "input_kind": e.input_kind, "output_kind": e.output_kind,
            "attrs": serde_json::from_str::<Value>(&e.attrs).unwrap_or(json!({})),
            "offerings": offerings, "aliases": aliases,
        }))
    }

    /// The id that answers on one address, without building any JSON.
    pub fn entity_at(&self, head: &str, tail: &str) -> Result<Option<String>> {
        Ok(self
            .entity_addresses()?
            .into_iter()
            .find(|(_, _, h, t)| h == head && t == tail)
            .map(|(id, _, _, _)| id))
    }

    pub fn export_json(&self) -> Result<Value> {
        let mut providers = Vec::new();
        for p in self.providers()? {
            let docs = self.docs_of(&p.id)?;
            providers.push(
                json!({"id": p.id, "name": p.name, "url": p.url, "kind": p.kind, "docs": docs}),
            );
        }
        let mut entities = Vec::new();
        for register in ["model", "tool", "agent", "subscription"] {
            for e in self.entities(register)? {
                let offerings: Vec<Value> = self
                    .offerings_of(&e.id)?
                    .into_iter()
                    .map(|o| {
                        let metrics: Vec<Value> = self
                            .current_metrics(o.offering_id)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|m| {
                                json!({"metric": m.metric, "value": m.value, "basis": m.basis,
                                       "source": m.source_url, "taken_at": m.taken_at})
                            })
                            .collect();
                        let components: Vec<Value> = o
                            .components
                            .iter()
                            .map(|c| {
                                json!({"dimension": c.dimension, "micros_per_unit": c.micros_per_unit,
                                       "basis": c.basis, "source": c.source_url, "taken_at": c.taken_at})
                            })
                            .collect();
                        json!({"provider_id": o.provider_id, "provider": o.provider_name,
                               "way": o.way, "variant": o.variant, "status": o.status,
                               "components": components, "metrics": metrics})
                    })
                    .collect();
                let aliases: Vec<Value> = self
                    .aliases_of(&e.id)?
                    .into_iter()
                    .map(|(source, alias)| json!({"source": source, "alias": alias}))
                    .collect();
                entities.push(json!({
                    "benchmarks": self.benchmarks_of(&e.id)?,
                    "docs": self.docs_of(&e.id)?,
                    "id": e.id, "register": e.register, "name": e.name, "maker": e.maker,
                    "family": e.family, "version": e.version, "derived_from": e.derived_from,
                    "input_kind": e.input_kind, "output_kind": e.output_kind,
                    "attrs": serde_json::from_str::<Value>(&e.attrs).unwrap_or(json!({})),
                    "offerings": offerings, "aliases": aliases,
                }));
            }
        }
        Ok(json!({"providers": providers, "entities": entities}))
    }

    // ---- addresses -------------------------------------------------------
    // A page is addressed by what it is, not by the id a crawler happened to
    // mint: /index/anthropic for the company, /index/anthropic/claude-opus-5
    // for the thing it makes. The first segment is a company or one of the
    // words the index keeps for its own hubs, so the two can never collide.

    /// Every company, and the address it answers on.
    pub fn provider_addresses(&self) -> Result<Vec<(String, String, String)>> {
        let mut out = Vec::new();
        let mut q = self.conn.prepare("SELECT id, name FROM providers")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            let s = address_slug(&name);
            out.push((id, name, s));
        }
        Ok(out)
    }

    /// Every product, and the two segments it answers on. A product whose
    /// maker nobody can name lives under "commons": the market sells web
    /// search, and no one company makes it.
    pub fn entity_addresses(&self) -> Result<Vec<(String, String, String, String)>> {
        let makers: HashMap<String, String> = self
            .provider_addresses()?
            .into_iter()
            .map(|(id, _, s)| (id, s))
            .collect();
        let mut out = Vec::new();
        let mut q = self
            .conn
            .prepare("SELECT id, name, COALESCE(maker,'') FROM entities ORDER BY name")?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (id, name, maker) = row?;
            let head = makers
                .get(&maker)
                .cloned()
                .unwrap_or_else(|| "commons".into());
            // A name in a script the slug cannot carry reduces to nothing,
            // and an empty second segment is an address that 404s. The
            // catalogue's own id stands in, never a blank.
            let mut tail = address_slug(&name);
            if tail.is_empty() {
                tail = address_slug(id.strip_prefix("ent_").unwrap_or(&id));
            }
            out.push((id, name, head, tail));
        }
        Ok(out)
    }

    /// What a company's page holds: what it makes, and what it sells that
    /// somebody else made. A company can be either without being both.
    pub fn provider_page(&self, id: &str) -> Result<Value> {
        let p = self
            .providers()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("no such provider"))?;
        let mut makes = Vec::new();
        let mut sells = Vec::new();
        for (eid, name, head, tail) in self.entity_addresses()? {
            let (register, maker): (String, String) = self.conn.query_row(
                "SELECT register, COALESCE(maker,'') FROM entities WHERE id=?1",
                params![eid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let row = json!({"id": eid, "name": name, "register": register,
                             "href": format!("/index/{head}/{tail}")});
            if maker == id {
                makes.push(row.clone());
            }
            let mine: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM offerings WHERE entity_id=?1 AND provider_id=?2",
                params![eid, id],
                |r| r.get(0),
            )?;
            if mine > 0 && maker != id {
                sells.push(row);
            }
        }
        // Venture money, where we have read a round for it. A company can be
        // in the catalogue with nothing priced and still be worth a page, and
        // this is usually the reason why.
        let (raised, rounds, source): (Option<i64>, Option<i64>, Option<String>) = self
            .conn
            .query_row(
                "SELECT raised, rounds, raised_source FROM providers WHERE id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or((None, None, None));
        // Who paid, and — if this company is itself a fund — who it paid.
        // One table read from both ends, alphabetically, because a list of a
        // hundred names is only usable if it is in an order.
        let mut q = self.conn.prepare(
            "SELECT f.name, f.id FROM investments i JOIN providers f ON f.id = i.fund_id
              WHERE i.company_id = ?1 ORDER BY f.name COLLATE NOCASE",
        )?;
        let backers: Vec<Value> = q
            .query_map(params![id], |r| {
                let n: String = r.get(0)?;
                Ok(json!({"name": n.clone(), "id": r.get::<_, String>(1)?,
                          "href": format!("/index/{}", address_slug(&n))}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut q = self.conn.prepare(
            "SELECT c.name, c.id FROM investments i JOIN providers c ON c.id = i.company_id
              WHERE i.fund_id = ?1 ORDER BY c.name COLLATE NOCASE",
        )?;
        let portfolio: Vec<Value> = q
            .query_map(params![id], |r| {
                let n: String = r.get(0)?;
                Ok(json!({"name": n.clone(), "id": r.get::<_, String>(1)?,
                          "href": format!("/index/{}", address_slug(&n))}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(json!({
            "kind": "provider", "id": p.id, "name": p.name, "url": p.url,
            "provider_kind": p.kind, "href": format!("/index/{}", address_slug(&p.name)),
            "docs": self.docs_of(&p.id)?, "makes": makes, "resells": sells,
            "raised": raised, "rounds": rounds, "raised_source": source,
            "backers": backers, "portfolio": portfolio,
        }))
    }

    /// What a board's page holds: who runs it, and everyone it has ranked.
    pub fn board_page(&self, suite: &str) -> Result<Value> {
        let (name, measurer, url, metric, lower): (String, String, String, String, i64) =
            self.conn.query_row(
                "SELECT name, COALESCE(measurer,''), COALESCE(url,''), COALESCE(metric,''), \
                        COALESCE(lower_is_better,0) FROM suites WHERE id=?1",
                params![suite],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?;
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();
        let mut q = self.conn.prepare(
            "SELECT b.entity_id, b.metric, b.value, b.rank, b.out_of, b.taken_at \
               FROM benchmarks b \
              WHERE b.suite=?1 \
                AND b.id=(SELECT id FROM benchmarks x \
                           WHERE x.entity_id=b.entity_id AND x.suite=b.suite AND x.metric=b.metric \
                           ORDER BY x.taken_at DESC, (x.rank IS NULL), x.rank, x.id DESC LIMIT 1) \
              ORDER BY b.rank IS NULL, b.rank, b.value DESC",
        )?;
        let rows: Vec<Value> = q
            .query_map(params![suite], |r| {
                let eid: String = r.get(0)?;
                Ok(json!({
                    "entity": eid, "metric": r.get::<_, String>(1)?, "value": r.get::<_, f64>(2)?,
                    "rank": r.get::<_, Option<i64>>(3)?, "out_of": r.get::<_, Option<i64>>(4)?,
                    "taken_at": r.get::<_, String>(5)?,
                }))
            })?
            .collect::<std::result::Result<_, _>>()?;
        // Two sources reporting one metric in different scales (91.16 vs
        // 0.9116) both persist, so one board printed some rows as percentages
        // and some as proportions — a 100x-wrong number for every row on the
        // minority scale. When a board genuinely mixes — some values clearly
        // percentages (>1.5) and some clearly proportions (<=1) — lift the
        // proportion rows to the percentage scale so the board reads on one.
        // A board wholly within [0,1.5] is left untouched: it is a real
        // proportion metric, not a mix.
        let vals: Vec<f64> = rows.iter().filter_map(|v| v["value"].as_f64()).collect();
        let mixed = vals.iter().any(|&x| x > 1.5) && vals.iter().any(|&x| x <= 1.0);
        let standings: Vec<Value> = rows
            .into_iter()
            .map(|mut v| {
                if mixed {
                    if let Some(x) = v["value"].as_f64() {
                        if x <= 1.0 {
                            v["value"] = json!((x * 100.0 * 1000.0).round() / 1000.0);
                            v["rescaled"] = json!(true);
                        }
                    }
                }
                let eid = v["entity"].as_str().unwrap_or("").to_string();
                if let Some((name, href)) = addr.get(&eid) {
                    v["name"] = json!(name);
                    v["href"] = json!(href);
                }
                v
            })
            .collect();
        Ok(json!({
            "kind": "board", "id": suite, "name": name, "measurer": measurer,
            "url": url, "metric": metric, "lower_is_better": lower == 1,
            "href": format!("/index/board/{}", address_slug(suite)), "standings": standings,
        }))
    }

    /// Everything the catalogue holds that answers to one word — a task it
    /// does, or a licence it is published under.
    pub fn facet_page(&self, facet: &str, value: &str) -> Result<Value> {
        let field = match facet {
            "task" => "$.tasks",
            "licence" => "$.license",
            _ => anyhow::bail!("no such facet"),
        };
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();
        let mut q = self.conn.prepare(&format!(
            "SELECT id, name, register, json_extract(attrs,'$.params') FROM entities \
              WHERE json_extract(attrs, '{field}') LIKE ?1 ORDER BY name"
        ))?;
        // The address carries the slug, the row carries the value the maker
        // wrote: "apache-2-0" in a URL is "apache-2.0" in the catalogue.
        let value = if facet == "task" {
            value.to_string()
        } else {
            let mut q = self.conn.prepare(
                "SELECT DISTINCT json_extract(attrs,'$.license') FROM entities \
                  WHERE json_extract(attrs,'$.license') IS NOT NULL",
            )?;
            let found = q
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .find(|v| address_slug(v) == value);
            match found {
                Some(v) => v,
                None => return Ok(json!({"kind": facet, "id": value, "members": []})),
            }
        };
        let needle = if facet == "task" {
            format!("%\"{value}\"%")
        } else {
            value.clone()
        };
        let members: Vec<Value> = q
            .query_map(params![needle], |r| {
                let id: String = r.get(0)?;
                let href = addr.get(&id).map(|(_, h)| h.clone()).unwrap_or_default();
                Ok(json!({"id": id, "name": r.get::<_, String>(1)?,
                          "register": r.get::<_, String>(2)?, "href": href,
                          // asked for in the same statement: a second query on
                          // a connection already stepping one quietly fails
                          "params": r.get::<_, Option<i64>>(3)?}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(json!({
            "kind": facet, "id": value, "name": value,
            "href": format!("/index/{facet}/{}", address_slug(&value)), "members": members,
        }))
    }

    /// Every address the index answers on, for the sitemap.
    /// When each entity's page last gained a fact: the newest of its price,
    /// standing and document dates. Only pages with a true data date appear;
    /// a sitemap lastmod that lies is worse than none, so nothing is padded.
    pub fn address_dates(&self) -> Result<HashMap<String, String>> {
        let mut dates: HashMap<String, String> = HashMap::new();
        for sql in [
            "SELECT o.entity_id, MAX(p.taken_at) FROM prices p \
              JOIN offerings o ON o.id = p.offering_id GROUP BY 1",
            "SELECT entity_id, MAX(taken_at) FROM benchmarks GROUP BY 1",
            "SELECT subject, MAX(taken_at) FROM docs GROUP BY 1",
        ] {
            let mut q = self.conn.prepare(sql)?;
            let rows: Vec<(String, Option<String>)> = q
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (id, d) in rows {
                let Some(d) = d else { continue };
                let e = dates.entry(id).or_default();
                if d > *e {
                    *e = d;
                }
            }
        }
        let mut out: HashMap<String, String> = HashMap::new();
        for (id, _, head, tail) in self.entity_addresses()? {
            if let Some(d) = dates.get(&id) {
                out.insert(format!("/index/{head}/{tail}"), d.clone());
            }
        }
        Ok(out)
    }

    pub fn all_addresses(&self) -> Result<Vec<String>> {
        let mut out = vec!["/index".to_string(), "/index/1dollar".to_string(),
                           "/index/tech".to_string(), "/index/startups".to_string(),
                           "/index/sizes".to_string()];
        for t in self.terms()? {
            out.push(format!("/index/tech/{}", t["slug"].as_str().unwrap_or("")));
        }
        // Each hub runs a hundred to a page, and every page is its own address.
        for (hub, register) in [("models", "model"), ("tools", "tool"), ("agents", "agent"),
                                ("subscriptions", "subscription")] {
            let n: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE register=?1",
                params![register],
                |r| r.get(0),
            )?;
            for p in 1..=((n as usize).div_ceil(100).max(1)) {
                out.push(if p == 1 {
                    format!("/index/{hub}")
                } else {
                    format!("/index/{hub}/{p}")
                });
            }
        }
        let live = self
            .provider_addresses()?
            .into_iter()
            .filter(|(id, _, _)| !self.provider_is_empty(id).unwrap_or(false))
            .count();
        for p in 1..=(live.div_ceil(100).max(1)) {
            out.push(if p == 1 {
                "/index/providers".to_string()
            } else {
                format!("/index/providers/{p}")
            });
        }
        // A company that neither makes nor sells anything has a page with
        // almost nothing on it — unless somebody wrote down why the catalogue
        // keeps it. "Harvey: legal work at the largest firms, priced by
        // contract, nothing published" is a fact nobody else publishes and is
        // worth an address. "coding", which is a name somebody typed, is not.
        for (id, _, s) in self.provider_addresses()? {
            if self.provider_is_empty(&id)? {
                // A company with nothing priced earns an address by there
                // being something to read: a signed note, a venture mark, or
                // a description of what it does. A name and nothing else does
                // not, and never has.
                let justified: bool = self.conn.query_row(
                    "SELECT COALESCE(notes,'') <> '' OR backing IS NOT NULL \
                            OR raised IS NOT NULL \
                            OR EXISTS(SELECT 1 FROM docs d WHERE d.subject = providers.id \
                                       AND d.kind = 'description') \
                            OR EXISTS(SELECT 1 FROM investments v \
                                       WHERE v.fund_id = providers.id \
                                          OR v.company_id = providers.id) \
                       FROM providers WHERE id=?1",
                    params![&id],
                    |r| r.get(0),
                )?;
                if !justified {
                    continue;
                }
            }
            out.push(format!("/index/{s}"));
        }
        for (_, _, head, tail) in self.entity_addresses()? {
            out.push(format!("/index/{head}/{tail}"));
        }
        let mut q = self.conn.prepare("SELECT DISTINCT suite FROM benchmarks")?;
        for row in q.query_map([], |r| r.get::<_, String>(0))? {
            out.push(format!("/index/board/{}", address_slug(&row?)));
        }
        let mut q = self.conn.prepare(
            "SELECT DISTINCT json_extract(attrs,'$.license') FROM entities \
              WHERE json_extract(attrs,'$.license') IS NOT NULL",
        )?;
        for row in q.query_map([], |r| r.get::<_, String>(0))? {
            out.push(format!("/index/licence/{}", address_slug(&row?)));
        }
        // Every list that has at least one member, single and paired. A list
        // of three is a list somebody can share; only an empty one is useless.
        let tasks = self.task_tags()?;
        for t in &tasks {
            out.push(format!("/index/for/{}", address_slug(t)));
        }
        for (i, o) in self.modality_pairs()? {
            let slug = format!("{}-to-{}", address_slug(&i), address_slug(&o));
            if self.list_page(&[("does", &slug)])?.is_some() {
                out.push(format!("/index/does/{slug}"));
            }
        }
        for (fam, _, _) in LICENCE_FAMILIES {
            if self.list_page(&[("licence", fam)])?.is_some() {
                out.push(format!("/index/licence/{fam}"));
            }
        }
        for t in &tasks {
            let ts = address_slug(t);
            for (fam, _, _) in LICENCE_FAMILIES {
                if self
                    .list_page(&[("for", &ts), ("licence", fam)])?
                    .is_some()
                {
                    out.push(format!("/index/for/{ts}/licence/{fam}"));
                }
            }
        }
        for (band, _, _) in MEMORY_BANDS {
            if self.list_page(&[("local", band)])?.is_some() {
                out.push(format!("/index/local/{band}"));
                for t in &tasks {
                    let ts = address_slug(t);
                    if self.list_page(&[("local", band), ("for", &ts)])?.is_some() {
                        out.push(format!("/index/local/{band}/for/{ts}"));
                    }
                }
            }
        }
        out.push("/index/waiting".into());
        out.push("/index/free".into());
        for kind in ["models", "tools", "agents", "subscriptions"] {
            out.push(format!("/index/free/{kind}"));
        }
        out.push("/index/top".into());
        for n in crate::top::NICHES {
            if self.top_page(n.key)?.is_some() {
                out.push(format!("/index/top/{}", n.key));
            }
        }
        out.push("/index/lists".into());
        out.push("/index/coverage".into());
        // "proprietary" is both a licence family and a licence, so it was
        // listed twice. One address, one entry.
        let mut seen = std::collections::BTreeSet::new();
        out.retain(|a| seen.insert(a.clone()));
        Ok(out)
    }

    /// The register of one product, for the hub lists.
    pub fn register_of(&self, id: &str) -> Result<String> {
        Ok(self
            .conn
            .query_row("SELECT register FROM entities WHERE id=?1", params![id], |r| r.get(0))?)
    }

    /// Every board the catalogue has a standing on.
    pub fn all_suite_ids(&self) -> Result<Vec<String>> {
        let mut q = self.conn.prepare("SELECT DISTINCT suite FROM benchmarks ORDER BY suite")?;
        let out = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// The whole catalogue as a list can afford: a name, an address, what it
    /// is, who made it, how many sell it, and the one figure the row prints.
    /// The browsing page used to carry every price of every seller to do the
    /// same job — five and a half megabytes to draw forty rows.
    ///
    /// Built with a handful of set queries rather than a few per row. Asking
    /// per row cost eight seconds; the same answers grouped cost a moment.
    /// Where two sources report the same seller's same rate differently.
    ///
    /// One of them is wrong and the catalogue cannot tell which, so it shows
    /// the seller's own page where there is one and says on the row that the
    /// figure is disputed. Silently picking a side and printing it as a fact
    /// is the version of this that loses a reader's trust once and for good.
    pub fn disputed_rates(&self, entity_id: &str) -> Result<Vec<Value>> {
        let mut q = self.conn.prepare(
            "WITH latest AS ( \
                SELECT x.offering_id, x.dimension, x.source_url, x.micros_per_unit \
                  FROM prices x \
                  JOIN (SELECT offering_id, dimension, source_url, MAX(id) mid \
                          FROM prices GROUP BY 1,2,3) k ON k.mid = x.id) \
             SELECT p.name, l.dimension, MIN(l.micros_per_unit), MAX(l.micros_per_unit), \
                    COUNT(DISTINCT l.source_url) \
               FROM latest l \
               JOIN offerings o ON o.id = l.offering_id \
               JOIN providers p ON p.id = o.provider_id \
              WHERE o.entity_id = ?1 \
              GROUP BY l.offering_id, l.dimension \
             HAVING COUNT(DISTINCT l.micros_per_unit) > 1 \
              ORDER BY MAX(l.micros_per_unit) * 1.0 / MIN(l.micros_per_unit) DESC",
        )?;
        let rows = q.query_map(params![entity_id], |r| {
            Ok(json!({
                "seller": r.get::<_, String>(0)?, "dimension": r.get::<_, String>(1)?,
                "low": r.get::<_, i64>(2)?, "high": r.get::<_, i64>(3)?,
                "sources": r.get::<_, i64>(4)?,
            }))
        })?;
        Ok(rows.filter_map(|x| x.ok()).collect())
    }

    pub fn find_index(&self) -> Result<Value> {
        let makers: HashMap<String, String> = self
            .provider_addresses()?
            .into_iter()
            .map(|(id, name, _)| (id, name))
            .collect();

        let mut sellers: HashMap<String, i64> = HashMap::new();
        let mut q = self
            .conn
            .prepare("SELECT entity_id, COUNT(DISTINCT provider_id) FROM offerings GROUP BY 1")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (id, n) = row?;
            sellers.insert(id, n);
        }

        // The cheapest current rate in every dimension an entity is sold by,
        // and the day any of it was last read. The `free` lane is left out on
        // purpose: it is not a cheaper way to buy the same thing but a
        // different arrangement, with a cap and an end date, and letting it
        // set the floor made GLM 5.2 read "$0 → $0" in the list.
        let mut rates: HashMap<String, Vec<(String, i64)>> = HashMap::new();
        let mut dear: HashMap<(String, String), i64> = HashMap::new();
        let mut fresh: HashMap<String, String> = HashMap::new();
        let mut q = self.conn.prepare(
            "SELECT o.entity_id, o.id, l.dimension, l.micros_per_unit, l.taken_at, \
                    l.source_url, COALESCE(p.url,'') \
               FROM prices l \
               JOIN offerings o ON o.id = l.offering_id \
               JOIN providers p ON p.id = o.provider_id \
               JOIN (SELECT offering_id, dimension, source_url, MAX(id) mid \
                       FROM prices GROUP BY 1,2,3) k ON k.mid = l.id \
              WHERE COALESCE(o.variant,'') = '' AND o.status = 'live'",
        )?;
        // Every source's latest word on every offering, so the figure shown is
        // chosen by a rule rather than by which collector wrote last. 842
        // offering-and-dimension pairs are reported by more than one source
        // and 213 of them disagree — sometimes by a factor of three — and the
        // winner used to be an accident of the order of a shell script.
        let mut heard: HashMap<(i64, String), Vec<(u8, i64, String)>> = HashMap::new();
        let mut owner: HashMap<i64, String> = HashMap::new();
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })? {
            let (eid, oid, dim, micros, taken, src, home) = row?;
            owner.insert(oid, eid);
            heard
                .entry((oid, dim))
                .or_default()
                .push((authority(&src, &home), micros, taken));
        }

        // One figure per offering and dimension, and how many sources stood
        // behind it. Disagreement is counted rather than smoothed away: it is
        // the fact a reader most needs and the one nobody else reports.
        // The agreement reported is the one behind the figure the row prints —
        // the cheapest seller's — not a sum across forty sellers, which would
        // say "89 of 89" and mean nothing.
        let mut agreement: HashMap<(String, String), (i64, i64)> = HashMap::new();
        let mut cheapest: HashMap<(String, String), i64> = HashMap::new();
        for ((oid, dim), mut says) in heard {
            // The seller's own page is the authority — but only while it is
            // current. Once it is more than 45 days behind the freshest word
            // on this offering, a broken seller-page collector would pin a
            // withdrawn rate forever, so past that gap the freshest figure
            // wins regardless of who said it. Within the window the seller's
            // page still outranks a third-party catalogue.
            let newest = says.iter().map(|s| s.2.clone()).max().unwrap_or_default();
            let effective = |a: &(u8, i64, String)| -> u8 {
                if a.0 >= 2 && days_between(&a.2, &newest) > 45 { 0 } else { a.0 }
            };
            says.sort_by(|a, b| effective(b).cmp(&effective(a)).then(b.2.cmp(&a.2)));
            let (_, micros, taken) = says[0].clone();
            let Some(eid) = owner.get(&oid).cloned() else { continue };
            let key = (eid.clone(), dim.clone());

            let lowest = cheapest.entry(key.clone()).or_insert(micros);
            if micros <= *lowest {
                *lowest = micros;
                agreement.insert(
                    key.clone(),
                    (says.iter().filter(|s| s.1 == micros).count() as i64, says.len() as i64),
                );
            }
            rates.entry(eid.clone()).or_default().push((dim.clone(), micros));
            let d = dear.entry(key).or_insert(micros);
            if micros > *d {
                *d = micros;
            }
            let f = fresh.entry(eid).or_default();
            if taken > *f {
                *f = taken;
            }
        }

        // Which figure the row prints, when a thing is metered several ways.
        for v in rates.values_mut() {
            v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            v.dedup_by(|a, b| a.0 == b.0);      // the cheapest of each dimension
        }

        let order = |d: &str| match d {
            "mtok_in" => 0,
            "mtok_out" => 1,
            "image" => 2,
            "second" => 3,
            "minute" => 4,
            "call" => 5,
            "character" => 6,
            _ => 9,
        };

        // The best a thing has placed anywhere, and on how many boards it was
        // asked. A row that says what a thing costs but not whether it is any
        // good is half a row, and the catalogue now holds 5,304 standings that
        // were not reaching the list at all.
        let mut best: HashMap<String, (i64, i64, i64)> = HashMap::new();
        let mut q = self.conn.prepare(
            "SELECT b.entity_id, b.rank, b.out_of FROM benchmarks b \
              WHERE b.rank IS NOT NULL AND b.out_of > 1 \
                AND b.id=(SELECT MAX(id) FROM benchmarks x \
                           WHERE x.entity_id=b.entity_id AND x.suite=b.suite AND x.metric=b.metric)",
        )?;
        for row in q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })? {
            let (id, rank, out_of) = row?;
            let e = best.entry(id).or_insert((rank, out_of, 0));
            e.2 += 1;
            // A better placing is a smaller rank in a larger field; comparing
            // the share beaten rather than the rank keeps 3rd of 400 ahead of
            // 2nd of 5.
            let better = (out_of - rank) as f64 / (out_of - 1) as f64;
            let held = (e.1 - e.0) as f64 / (e.1 - 1).max(1) as f64;
            if better > held {
                e.0 = rank;
                e.1 = out_of;
            }
        }
        let mut boards_of: HashMap<String, i64> = HashMap::new();
        let mut q = self
            .conn
            .prepare("SELECT entity_id, COUNT(DISTINCT suite) FROM benchmarks GROUP BY 1")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (id, n) = row?;
            boards_of.insert(id, n);
        }

        let mut facts: HashMap<String, (String, String, String)> = HashMap::new();
        let mut q = self.conn.prepare(
            "SELECT id, register, COALESCE(maker,''), COALESCE(attrs,'{}') FROM entities",
        )?;
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (id, register, maker, attrs) = row?;
            facts.insert(id, (register, maker, attrs));
        }

        let mut rows = Vec::new();
        for (id, name, head, tail) in self.entity_addresses()? {
            let (register, maker, attrs) = facts.get(&id).cloned().unwrap_or_default();
            let attrs: Value = serde_json::from_str(&attrs).unwrap_or(json!({}));
            // A row should be usable where it stands. For anything metered by
            // tokens that means the pair a reader quotes, in and out; for
            // anything else sold by more than one company it means the range,
            // because the cheapest alone hides that the dearest is ten times it.
            let lead = rates.get(&id).and_then(|v| {
                v.iter()
                    .min_by_key(|(d, m)| (order(d), *m))
                    .map(|(d, m)| (d.clone(), *m))
            });
            let out_rate = lead.as_ref().filter(|(d, _)| d == "mtok_in").and_then(|_| {
                rates
                    .get(&id)?
                    .iter()
                    .find(|(d, _)| d == "mtok_out")
                    .map(|(_, m)| *m)
            });
            let dearest = lead.as_ref().and_then(|(d, _)| dear.get(&(id.clone(), d.clone())).copied());
            rows.push(json!({
                "n": name,
                "h": format!("/index/{head}/{tail}"),
                "r": register,
                "m": makers.get(&maker).cloned().unwrap_or_default(),
                "s": sellers.get(&id).copied().unwrap_or(0),
                "d": lead.as_ref().map(|(d, _)| d.clone()),
                "p": lead.as_ref().map(|(_, m)| *m),
                "o": out_rate,
                "x": dearest,
                "t": attrs["tasks"].clone(),
                "u": fresh.get(&id).cloned(),
                // What a plan allows, which for a subscription is the half of
                // the price a figure cannot carry. On any other row it is
                // absent and the seller count keeps its place.
                "lm": attrs["limits"].as_str(),
                // How many sources agreed on the figure this row prints, and
                // how many spoke at all. Two that disagree is the fact a
                // reader most needs and the one nobody else reports.
                "ca": lead.as_ref().and_then(|(d, _)| agreement.get(&(id.clone(), d.clone())))
                          .map(|(same, _)| *same),
                "cs": lead.as_ref().and_then(|(d, _)| agreement.get(&(id.clone(), d.clone())))
                          .map(|(_, all)| *all),
                "br": best.get(&id).map(|(r, _, _)| *r),
                "bf": best.get(&id).map(|(_, f, _)| *f),
                "bn": boards_of.get(&id).copied(),
            }));
        }

        let mut sold: HashMap<String, i64> = HashMap::new();
        let mut q = self
            .conn
            .prepare("SELECT provider_id, COUNT(DISTINCT entity_id) FROM offerings GROUP BY 1")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (id, n) = row?;
            sold.insert(id, n);
        }
        let mut made: HashMap<String, i64> = HashMap::new();
        let mut q = self
            .conn
            .prepare("SELECT maker, COUNT(*) FROM entities WHERE maker IS NOT NULL GROUP BY 1")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (id, n) = row?;
            made.insert(id, n);
        }
        let mut kinds: HashMap<String, String> = HashMap::new();
        let mut q = self.conn.prepare("SELECT id, kind FROM providers")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (id, k) = row?;
            kinds.insert(id, k);
        }
        // Which companies a reader can be offered. Once it was only those
        // selling something, which left fourteen hundred startups and every
        // fund unfindable: the search box could not name what the catalogue
        // held pages for. The rule is now the same one the sitemap uses —
        // a company is offered when it has a page worth opening.
        let mut has_page: std::collections::HashSet<String> = Default::default();
        let mut q = self.conn.prepare(
            "SELECT id FROM providers \
              WHERE COALESCE(notes,'') <> '' OR backing IS NOT NULL OR raised IS NOT NULL \
                 OR EXISTS(SELECT 1 FROM docs d WHERE d.subject = providers.id \
                            AND d.kind = 'description') \
                 OR EXISTS(SELECT 1 FROM investments v WHERE v.fund_id = providers.id \
                            OR v.company_id = providers.id)",
        )?;
        for row in q.query_map([], |r| r.get::<_, String>(0))? {
            has_page.insert(row?);
        }
        let mut providers = Vec::new();
        for (id, name, s) in self.provider_addresses()? {
            if sold.get(&id).copied().unwrap_or(0) == 0
                && made.get(&id).copied().unwrap_or(0) == 0
                && !has_page.contains(&id)
            {
                continue;
            }
            providers.push(json!({
                "n": name, "h": format!("/index/{s}"), "r": "provider",
                "k": kinds.get(&id).cloned().unwrap_or_default(),
                "s": sold.get(&id).copied().unwrap_or(0),
                "makes": made.get(&id).copied().unwrap_or(0),
            }));
        }
        Ok(json!({"things": rows, "companies": providers}))
    }

    /// Nothing made and nothing sold: a name, and no page worth serving.
    pub fn provider_is_empty(&self, id: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM offerings WHERE provider_id=?1) \
                  + (SELECT COUNT(*) FROM entities WHERE maker=?1)",
            params![id],
            |r| r.get(0),
        )?;
        Ok(n == 0)
    }

    /// What the catalogue holds, how fresh it is, and what it cannot say. A
    /// reference work that will not state its own gaps is asking to be
    /// trusted rather than checked.
    /// The last run of every self-check, for the coverage page.
    pub fn checks(&self) -> Result<Value> {
        let mut q = self.conn.prepare(
            "SELECT name, suite, blocking, findings, asks, ran_at FROM checks \
              ORDER BY suite, blocking DESC, name",
        )?;
        let rows: Vec<Value> = q
            .query_map([], |r| {
                Ok(json!({
                    "name": r.get::<_, String>(0)?,
                    "suite": r.get::<_, String>(1)?,
                    "blocking": r.get::<_, i64>(2)? == 1,
                    "findings": r.get::<_, i64>(3)?,
                    "asks": r.get::<_, String>(4)?,
                    "ran_at": r.get::<_, String>(5)?,
                }))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let ran: Option<String> = self
            .conn
            .query_row("SELECT MAX(ran_at) FROM checks", [], |r| r.get(0))
            .optional()?
            .flatten();
        let failed = rows.iter().filter(|c| c["findings"].as_i64().unwrap_or(0) > 0).count();
        Ok(json!({"checks": rows, "ran_at": ran, "failing": failed}))
    }

    /// What the catalogue knows exists and cannot price.
    ///
    /// Two very different things end up here and the difference is the whole
    /// point. Harvey and Sierra are among the largest companies in this
    /// market and publish no price at all; a connector nobody has heard of
    /// also publishes no price. Filing them together would say they are alike.
    ///
    /// So the room is ordered by corroboration: how many things in the
    /// catalogue name this company as their maker, and how many separate
    /// sources have written about it. A name the market keeps mentioning and
    /// nobody prices is a gap worth working; a name mentioned once is noise,
    /// and the page says which is which rather than hiding either.
    pub fn waiting(&self) -> Result<Value> {
        let mut q = self.conn.prepare(
            "SELECT p.id, p.name, p.kind, COALESCE(p.url,''), \
                    (SELECT COUNT(*) FROM entities e WHERE e.maker = p.id) makes, \
                    (SELECT COUNT(DISTINCT d.source_url) FROM docs d WHERE d.subject = p.id) said, \
                    COALESCE(p.notes,'') why \
               FROM providers p \
              WHERE NOT EXISTS (SELECT 1 FROM offerings o WHERE o.provider_id = p.id) \
              ORDER BY why = '' , makes + said DESC, p.name",
        )?;
        let companies: Vec<Value> = q
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                    "kind": r.get::<_, String>(2)?, "url": r.get::<_, String>(3)?,
                    "makes": r.get::<_, i64>(4)?, "said": r.get::<_, i64>(5)?,
                    "why": r.get::<_, String>(6)?,
                }))
            })?
            .collect::<std::result::Result<_, _>>()?;

        // A listing the collector could not place. Not a gap in the market —
        // a gap in our reading of it, and the operator's queue.
        let mut q = self.conn.prepare(
            "SELECT source, alias, last_seen FROM unmatched_listings ORDER BY last_seen DESC, alias",
        )?;
        let unbound: Vec<Value> = q
            .query_map([], |r| {
                Ok(json!({"source": r.get::<_, String>(0)?, "alias": r.get::<_, String>(1)?,
                          "seen": r.get::<_, String>(2)?}))
            })?
            .collect::<std::result::Result<_, _>>()?;

        // A card with nothing on it: no price, no standing, no description.
        let mut q = self.conn.prepare(
            "SELECT e.id, e.name, e.register FROM entities e \
              WHERE NOT EXISTS (SELECT 1 FROM offerings o JOIN prices x ON x.offering_id=o.id \
                                 WHERE o.entity_id=e.id) \
                AND NOT EXISTS (SELECT 1 FROM benchmarks b WHERE b.entity_id=e.id) \
              ORDER BY e.name",
        )?;
        let bare: Vec<Value> = q
            .query_map([], |r| {
                Ok(json!({"id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)?,
                          "register": r.get::<_, String>(2)?}))
            })?
            .collect::<std::result::Result<_, _>>()?;

        Ok(json!({
            "kind": "waiting", "href": "/index/waiting",
            "companies": companies, "unbound": unbound, "bare": bare,
            "read": self.last_read()?,
        }))
    }

    pub fn coverage(&self) -> Result<Value> {
        let one = |sql: &str| -> Result<i64> { Ok(self.conn.query_row(sql, [], |r| r.get(0))?) };
        let pairs = |sql: &str| -> Result<Vec<Value>> {
            let mut q = self.conn.prepare(sql)?;
            let rows = q
                .query_map([], |r| {
                    Ok(json!({"k": r.get::<_, String>(0)?, "n": r.get::<_, i64>(1)?}))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        };
        Ok(json!({
            "entities": one("SELECT COUNT(*) FROM entities")?,
            "providers": one("SELECT COUNT(*) FROM providers")?,
            "ways": one("SELECT COUNT(*) FROM offerings")?,
            "figures": one("SELECT COUNT(*) FROM prices")?,
            "standings": one("SELECT COUNT(*) FROM benchmarks")?,
            "boards": one("SELECT COUNT(DISTINCT suite) FROM benchmarks")?,
            "texts": one("SELECT COUNT(*) FROM docs")?,
            "aliases": one("SELECT COUNT(*) FROM aliases")?,
            "by_register": pairs("SELECT register, COUNT(*) FROM entities GROUP BY 1 ORDER BY 2 DESC")?,
            "by_kind": pairs("SELECT kind, COUNT(*) FROM providers GROUP BY 1 ORDER BY 2 DESC")?,
            "by_way": pairs("SELECT way, COUNT(*) FROM offerings GROUP BY 1 ORDER BY 2 DESC")?,
            "by_task": pairs(
                "SELECT j.value, COUNT(*) FROM entities e, \
                        json_each(json_extract(e.attrs,'$.tasks')) j GROUP BY 1 ORDER BY 2 DESC")?,
            "by_licence": pairs(
                "SELECT json_extract(attrs,'$.license'), COUNT(*) FROM entities \
                  WHERE json_extract(attrs,'$.license') IS NOT NULL GROUP BY 1 ORDER BY 2 DESC")?,
            "read_on": pairs(
                "SELECT taken_at, COUNT(*) FROM prices GROUP BY 1 ORDER BY 1 DESC LIMIT 10")?,
            "gaps": [
                {"n": one("SELECT COUNT(*) FROM entities e WHERE NOT EXISTS \
                    (SELECT 1 FROM offerings o JOIN prices x ON x.offering_id=o.id \
                      WHERE o.entity_id=e.id)")?,
                 "what": "things nobody in here publishes a price for — a seat licence, \
                          a quote on request, or open weights no one hosts"},
                {"n": one("SELECT COUNT(*) FROM entities WHERE \
                    json_extract(attrs,'$.license') IS NULL")?,
                 "what": "things whose licence we could not read off a model card, mostly \
                          makers who publish both open and closed models"},
                {"n": one("SELECT COUNT(*) FROM entities e WHERE NOT EXISTS \
                    (SELECT 1 FROM docs d WHERE d.subject=e.id AND d.kind='description')")?,
                 "what": "things with no description in the maker's own words yet"},
                {"n": one("SELECT COUNT(*) FROM entities e WHERE NOT EXISTS \
                    (SELECT 1 FROM benchmarks b WHERE b.entity_id=e.id)")?,
                 "what": "things no board in here has ranked"},
                {"n": one("SELECT COUNT(*) FROM providers p WHERE NOT EXISTS \
                    (SELECT 1 FROM offerings o WHERE o.provider_id=p.id) AND NOT EXISTS \
                    (SELECT 1 FROM entities e WHERE e.maker=p.id)")?,
                 "what": "companies we know exist but hold nothing for yet, so they have \
                          no page"},
            ],
        }))
    }

    /// A list is whatever a reader would type into a search box, and those
    /// phrases are compound: "open voice models" is a licence and a task at
    /// once. So one machine takes a set of constraints rather than one facet,
    /// and a new axis — a country, when we have crawled for it — is a row in
    /// the match below, not another page builder.
    ///
    /// There is no minimum size. A list of three is a list somebody can share,
    /// and in six months it is thirty; the only bar is that it not be empty.
    /// What somebody will let you have without paying, and on what terms.
    ///
    /// Three different promises, kept apart because confusing them is how a
    /// reader ends up with a bill. A seller charging nothing is not the same
    /// as a licence charging nothing: the first can stop tomorrow and comes
    /// with a queue, the second cannot be withdrawn but leaves you holding
    /// the hardware. And a licence that forbids selling the output is free
    /// only until the thing you build starts earning.
    /// Every plan that gives you a thing, for its own card. Two ways a plan
    /// can do that: it was written for the product — Cursor Pro is a plan for
    /// Cursor — or it names the thing among what it reaches, which is how a
    /// model comes to be inside a subscription that is not about it. A card
    /// for a product sold only by the month would otherwise say nothing at
    /// all about what it costs.
    pub fn plans_for(&self, entity_id: &str) -> Result<Vec<Value>> {
        let name: String = self
            .conn
            .query_row("SELECT name FROM entities WHERE id=?1", params![entity_id], |r| r.get(0))
            .optional()?
            .unwrap_or_default();
        let mut q = self.conn.prepare(
            "SELECT s.id, s.name, json_extract(s.attrs,'$.limits'), \
                    MIN(p.micros_per_unit), MAX(pr.name) \
               FROM entities s \
               JOIN offerings o ON o.entity_id = s.id \
               JOIN current_prices p ON p.offering_id = o.id AND p.dimension = 'month' \
               JOIN providers pr ON pr.id = o.provider_id \
              WHERE s.register = 'subscription' \
                AND (s.derived_from = ?1 \
                     OR (?2 <> '' AND json_extract(s.attrs,'$.includes') LIKE '%\"' || ?2 || '\"%')) \
              GROUP BY s.id ORDER BY MIN(p.micros_per_unit)",
        )?;
        let addr: HashMap<String, String> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, _, head, tail)| (id, format!("/index/{head}/{tail}")))
            .collect();
        let rows = q.query_map(params![entity_id, name], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "limits": r.get::<_, Option<String>>(2)?,
                "month": r.get::<_, i64>(3)?,
                "seller": r.get::<_, String>(4)?,
            }))
        })?;
        Ok(rows
            .filter_map(|x| x.ok())
            .map(|mut v| {
                if let Some(h) = addr.get(v["id"].as_str().unwrap_or("")) {
                    v["href"] = json!(h);
                }
                v
            })
            .collect())
    }

    /// Models that place on a board and cost under a dollar to run.
    ///
    /// Two conditions, both hard. It must stand somewhere — a cheap model
    /// nobody has measured is a cheap unknown, and the point of the list is
    /// that these work. And somebody must sell its output for less than a
    /// dollar the million tokens, on the standard lane: a free tier is an
    /// allowance that can be withdrawn tomorrow and a batch queue is a
    /// different product, so neither earns a place here.
    /// How big a model has to be to lead its category for under a dollar.
    ///
    /// The four categories are the directions the reader asked for — General,
    /// Reasoning, Coding, Agentic — and each is judged on its own boards,
    /// the same sets the Top page's picks are judged on. A model is in a
    /// category because it stands on that category's boards; the top three
    /// are taken and their parameter counts averaged.
    ///
    /// Most of the leaders at this price are closed and publish no size, so
    /// leaving them out would answer a different question — how big are the
    /// best *open* cheap models — and that is not the question. Instead an
    /// unpublished size is estimated as the median of the three models
    /// nearest it in standing, in the same category, that do publish one.
    /// The estimate is marked as an estimate everywhere it is shown.
    pub fn dollar_by_family(&self, ceiling: i64) -> Result<Vec<Value>> {
        // The current rate, not the cheapest ever recorded: `prices` is
        // append-only, so a bare MIN over it advertises rates that were
        // withdrawn.
        let mut q = self.conn.prepare(
            "SELECT o.entity_id, MIN(cur.micros_per_unit)
               FROM offerings o JOIN current_prices cur ON cur.offering_id = o.id
              WHERE cur.dimension = 'mtok_out'
                AND COALESCE(o.variant,'') = ''
                AND o.status = 'live'
                AND cur.micros_per_unit > 0 AND cur.micros_per_unit < ?1
              GROUP BY o.entity_id",
        )?;
        let cheap: HashMap<String, i64> = q
            .query_map([ceiling], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(q);

        let mut q = self.conn.prepare(
            "SELECT id, name, CAST(json_extract(attrs,'$.params') AS INTEGER) FROM entities \
              WHERE register = 'model'",
        )?;
        let meta: HashMap<String, (String, Option<i64>)> = q
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<rusqlite::Result<_>>()?;
        drop(q);

        // Every cheap model that publishes a size, for the fallback estimate
        // in a category where none of them does.
        let everywhere: Vec<i64> = cheap
            .keys()
            .filter_map(|e| meta.get(e).and_then(|m| m.1))
            .collect();

        // The niche keys carry the boards; the titles are the reader's own
        // words for the four directions.
        let mut families: Vec<(&str, &str, Vec<&str>)> = Vec::new();
        for (key, title, list_href) in [
            ("chat", "General", "/index/for/chat"),
            ("reasoning", "Reasoning", "/index/for/reasoning"),
            ("code", "Coding", "/index/for/code"),
            ("agents", "Agentic", "/index/for/agents"),
        ] {
            let Some(n) = crate::top::NICHES.iter().find(|n| n.key == key) else { continue };
            families.push((title, list_href, n.boards.to_vec()));
        }

        let hrefs: HashMap<String, String> = self
            .entity_addresses()?
            .into_iter()
            .map(|(eid, _, head, tail)| (eid, format!("/index/{head}/{tail}")))
            .collect();

        let mut out = Vec::new();
        // Every model the four tops picked, once each — a model that leads
        // two categories is one model, not two data points.
        let mut picked: Vec<(String, i64, i64)> = Vec::new();
        for (family, list_href, boards) in families {
            let marks = vec!["?"; boards.len()].join(",");
            let mut q = self.conn.prepare(&format!(
                "SELECT entity_id, MIN(CAST(rank AS REAL)/out_of) FROM benchmarks \
                  WHERE suite IN ({marks}) AND rank IS NOT NULL AND out_of > 1 \
                  GROUP BY entity_id"
            ))?;
            let placed: Vec<(String, f64)> = q
                .query_map(rusqlite::params_from_iter(boards.iter()), |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            drop(q);

            let mut pool: Vec<(&String, f64, i64)> = placed
                .iter()
                .filter_map(|(e, share)| cheap.get(e).map(|px| (e, *share, *px)))
                .filter(|(e, _, _)| meta.contains_key(*e))
                .collect();
            pool.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap().then(a.2.cmp(&b.2)));
            let sized: Vec<(f64, i64)> = pool
                .iter()
                .filter_map(|(e, share, _)| meta[*e].1.map(|p| (*share, p)))
                .collect();

            let mut picks = Vec::new();
            let mut total = 0f64;
            let mut counted = 0f64;
            for (eid, share, px) in pool.iter().take(3) {
                let (name, published) = &meta[*eid];
                // A published size, or the median of the nearest-placed sized
                // models — first in this category, then across all cheap
                // models if none here publishes one. When even that is empty
                // (no cheap model anywhere states a size) there is nothing to
                // estimate from, so the pick is shown without a size rather
                // than indexing an empty list and panicking the whole page.
                let params: Option<i64> = match published {
                    Some(p) => Some(*p),
                    None => {
                        let mut near: Vec<i64> = if sized.is_empty() {
                            everywhere.clone()
                        } else {
                            let mut by = sized.clone();
                            by.sort_by(|a, b| (a.0 - share)
                                .abs()
                                .partial_cmp(&(b.0 - share).abs())
                                .unwrap());
                            by.iter().take(3).map(|x| x.1).collect()
                        };
                        near.sort_unstable();
                        near.get(near.len() / 2).copied()
                    }
                };
                let estimated = params.is_some() && published.is_none();
                if let Some(p) = params {
                    total += p as f64;
                    counted += 1.0;
                    if !picked.iter().any(|(i, _, _)| i == *eid) {
                        picked.push(((*eid).clone(), p, *px));
                    }
                }
                picks.push(serde_json::json!({
                    "id": eid, "name": name,
                    "href": hrefs.get(*eid).cloned().unwrap_or_default(),
                    "params": params, "estimated": estimated,
                    "out": px,
                    "rank_share": (share * 1000.0).round() / 1000.0,
                }));
            }
            if picks.is_empty() {
                continue;
            }
            out.push(serde_json::json!({
                "family": family,
                "list": list_href,
                "average_params": if counted > 0.0 { (total / counted).round() as i64 } else { 0 },
                "measured": pool.len(),
                "top": picks,
            }));
        }
        // The whole page in two figures: across every model the four tops
        // picked, how big on average, and what a million tokens of output
        // costs on average.
        if !picked.is_empty() {
            let n = picked.len() as f64;
            out.insert(0, serde_json::json!({
                "family": "Overall",
                "average_params":
                    (picked.iter().map(|(_, p, _)| *p as f64).sum::<f64>() / n).round() as i64,
                "average_out":
                    (picked.iter().map(|(_, _, px)| *px as f64).sum::<f64>() / n).round() as i64,
                "models": picked.len(),
            }));
        }
        Ok(out)
    }

    pub fn dollar_models(&self, ceiling: i64) -> Result<Vec<Value>> {
        let mut q = self.conn.prepare(
            "WITH cheap AS (
               SELECT o.entity_id AS eid,
                      MIN(l.micros_per_unit) AS out_low,
                      COUNT(DISTINCT o.provider_id) AS sellers
                 FROM current_prices l
                 JOIN offerings o ON o.id = l.offering_id
                WHERE l.dimension = 'mtok_out'
                  AND COALESCE(o.variant,'') = ''
                  AND o.status = 'live'
                  AND l.micros_per_unit > 0
                  AND l.micros_per_unit < ?1
                GROUP BY o.entity_id
             ),
             inrate AS (
               SELECT o.entity_id AS eid, MIN(l.micros_per_unit) AS in_low
                 FROM current_prices l
                 JOIN offerings o ON o.id = l.offering_id
                WHERE l.dimension = 'mtok_in' AND COALESCE(o.variant,'') = ''
                  AND o.status = 'live'
                  AND l.micros_per_unit > 0
                GROUP BY o.entity_id
             ),
             placed AS (
               SELECT b.entity_id AS eid,
                      COUNT(DISTINCT b.suite) AS boards,
                      MIN(CAST(b.rank AS REAL) / b.out_of) AS share
                 FROM benchmarks b
                WHERE b.rank IS NOT NULL AND b.out_of > 1
                GROUP BY b.entity_id
             )
             SELECT e.id, e.name, COALESCE(p.name,''), c.out_low, i.in_low,
                    c.sellers, pl.boards, pl.share
               FROM cheap c
               JOIN placed pl ON pl.eid = c.eid
               JOIN entities e ON e.id = c.eid
               LEFT JOIN inrate i ON i.eid = c.eid
               LEFT JOIN providers p ON p.id = e.maker
              WHERE e.register = 'model'
              ORDER BY pl.share, c.out_low",
        )?;
        let rows = q.query_map([ceiling], |r| {
            Ok(json!({
                "entity": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "maker": r.get::<_, String>(2)?,
                "out": r.get::<_, i64>(3)?,
                "in": r.get::<_, Option<i64>>(4)?,
                "sellers": r.get::<_, i64>(5)?,
                "boards": r.get::<_, i64>(6)?,
                "share": r.get::<_, f64>(7)?,
            }))
        })?;
        let mut out: Vec<Value> = rows.collect::<std::result::Result<_, _>>()?;
        // Where each one lives, from the one place that decides addresses.
        let addr: HashMap<String, String> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, _, head, tail)| (id, format!("/index/{head}/{tail}")))
            .collect();
        // The best place each one holds, said the way a board says it.
        for row in out.iter_mut() {
            let eid = row["entity"].as_str().unwrap_or("").to_string();
            let mut b = self.conn.prepare(
                "SELECT b.rank, b.out_of, COALESCE(s.name, b.suite)
                   FROM benchmarks b LEFT JOIN suites s ON s.id = b.suite
                  WHERE b.entity_id = ?1 AND b.rank IS NOT NULL AND b.out_of > 1
                  ORDER BY CAST(b.rank AS REAL) / b.out_of LIMIT 1",
            )?;
            if let Ok((rank, of, board)) = b.query_row([&eid], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
            }) {
                row["rank"] = json!(rank);
                row["field"] = json!(of);
                row["board"] = json!(board);
            }
            if let Some(href) = addr.get(&eid) {
                row["href"] = json!(href);
            }
        }
        Ok(out.into_iter().filter(|r| r.get("href").is_some()).collect())
    }

    /// The vocabulary, and one entry from it.
    ///
    /// Kept in the catalogue's own database because it is answering questions
    /// about the same market: what a token is, what 402 was reserved for, why
    /// two sellers of one model charge differently. Every entry points at a
    /// page that shows the thing rather than describing it.
    /// The people of the market, aggregated from the two facts the people
    /// job writes: who founded each company, who runs it. Ordered by how
    /// many companies a person touches, then by name.
    pub fn people(&self) -> Result<Vec<Value>> {
        let mut q = self.conn.prepare(
            "SELECT d.subject, d.field, d.text, p.name FROM docs d \
               JOIN providers p ON p.id = d.subject \
              WHERE d.kind='fact' AND d.field IN ('founded_by','led_by')",
        )?;
        let rows: Vec<(String, String, String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(q);
        // person name -> list of (provider id, provider name, role)
        let mut by: Vec<(String, Vec<(String, String, String)>)> = Vec::new();
        for (pid, field, text, pname) in rows {
            for person in text.split(", ").map(str::trim).filter(|s| !s.is_empty()) {
                let entry = match by.iter_mut().find(|(n, _)| n == person) {
                    Some(e) => e,
                    None => {
                        by.push((person.to_string(), Vec::new()));
                        by.last_mut().unwrap()
                    }
                };
                if !entry.1.iter().any(|(i, _, r)| *i == pid && *r == field) {
                    entry.1.push((pid.clone(), pname.clone(), field.clone()));
                }
            }
        }
        by.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        Ok(by
            .into_iter()
            .map(|(name, cs)| {
                json!({
                    "name": name,
                    "companies": cs.iter().map(|(id, n, role)| json!({
                        "id": id, "name": n, "role": role,
                        "href": format!("/index/{}", address_slug(n)),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect())
    }

    pub fn terms(&self) -> Result<Vec<Value>> {
        let mut q = self.conn.prepare(
            "SELECT slug, term, kind, short, body, also, see FROM terms ORDER BY term COLLATE NOCASE",
        )?;
        let rows = q.query_map([], |r| {
            Ok(json!({
                "slug": r.get::<_, String>(0)?,
                "term": r.get::<_, String>(1)?,
                "kind": r.get::<_, String>(2)?,
                "short": r.get::<_, String>(3)?,
                "body": r.get::<_, String>(4)?,
                "also": serde_json::from_str::<Value>(&r.get::<_, String>(5)?).unwrap_or(json!([])),
                "see": serde_json::from_str::<Value>(&r.get::<_, String>(6)?).unwrap_or(json!([])),
                "href": format!("/index/tech/{}", r.get::<_, String>(0)?),
            }))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn term(&self, slug: &str) -> Result<Option<Value>> {
        Ok(self.terms()?.into_iter().find(|t| t["slug"] == slug))
    }

    /// Companies we have read a venture round for.
    ///
    /// The mark is evidence, not judgement: a company earns it by an article
    /// stating a round, and a company with no round we could find does not
    /// get it — which is a fact about our reading, not about their balance
    /// sheet, and the page says so.
    pub fn startups(&self) -> Result<Vec<Value>> {
        let mut q = self.conn.prepare(
            "SELECT p.id, p.name, COALESCE(p.url,''), p.kind, p.raised, p.rounds,
                    COALESCE(p.raised_source,''),
                    (SELECT COUNT(DISTINCT o.entity_id) FROM offerings o
                      WHERE o.provider_id = p.id) AS sells,
                    (SELECT COUNT(*) FROM entities e WHERE e.maker = p.id) AS makes,
                    COALESCE(p.backing,''), COALESCE(p.status,''),
                    (SELECT d.text FROM docs d WHERE d.subject = p.id
                      AND d.kind = 'description' LIMIT 1)
               FROM providers p
              WHERE (p.raised IS NOT NULL AND p.raised > 0) OR p.backing IS NOT NULL
              ORDER BY COALESCE(p.raised, -1) DESC, p.name COLLATE NOCASE",
        )?;
        let rows = q.query_map([], |r| {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            Ok(json!({
                "id": id,
                "name": name.clone(),
                "url": r.get::<_, String>(2)?,
                "kind": r.get::<_, String>(3)?,
                "raised": r.get::<_, Option<i64>>(4)?,
                "rounds": r.get::<_, Option<i64>>(5)?,
                "source": r.get::<_, String>(6)?,
                "sells": r.get::<_, i64>(7)?,
                "makes": r.get::<_, i64>(8)?,
                "backing": r.get::<_, String>(9)?,
                "status": r.get::<_, String>(10)?,
                "what": r.get::<_, Option<String>>(11)?,
                "href": format!("/index/{}", address_slug(&name)),
            }))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Every model whose size somebody published, largest first.
    ///
    /// A model the catalogue cannot size is not here, and that is not an
    /// omission: a closed model's parameter count is a fact its maker chose
    /// not to state, and guessing it from a price would invent the one number
    /// this page exists to report.
    pub fn sized_models(&self) -> Result<Vec<Value>> {
        let addr: HashMap<String, String> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, _, head, tail)| (id, format!("/index/{head}/{tail}")))
            .collect();
        let mut q = self.conn.prepare(
            "SELECT e.id, e.name, COALESCE(p.name,''), \
                    CAST(json_extract(e.attrs,'$.params') AS INTEGER), \
                    COALESCE(json_extract(e.attrs,'$.params_read_from'),'a model card'), \
                    COALESCE(json_extract(e.attrs,'$.license'),''), \
                    COALESCE(json_extract(e.attrs,'$.context'),0), \
                    (SELECT COUNT(DISTINCT o.provider_id) FROM offerings o \
                      WHERE o.entity_id = e.id), \
                    (SELECT COUNT(DISTINCT b.suite) FROM benchmarks b \
                      WHERE b.entity_id = e.id), \
                    (SELECT MIN(pr.micros_per_unit) FROM current_prices pr \
                       JOIN offerings o2 ON o2.id = pr.offering_id \
                      WHERE o2.entity_id = e.id AND COALESCE(o2.variant,'') = '' \
                        AND o2.status = 'live' AND pr.dimension = 'mtok_in'), \
                    (SELECT MIN(pr.micros_per_unit) FROM current_prices pr \
                       JOIN offerings o2 ON o2.id = pr.offering_id \
                      WHERE o2.entity_id = e.id AND COALESCE(o2.variant,'') = '' \
                        AND o2.status = 'live' AND pr.dimension = 'mtok_out') \
               FROM entities e LEFT JOIN providers p ON p.id = e.maker \
              WHERE e.register = 'model' \
                AND json_extract(e.attrs,'$.params') IS NOT NULL \
              ORDER BY json_extract(e.attrs,'$.params') DESC, e.name COLLATE NOCASE",
        )?;
        let rows = q.query_map([], |r| {
            let id: String = r.get(0)?;
            let params: f64 = r.get::<_, i64>(3)? as f64;
            let billions = params / 1e9;
            let band = size_band(billions);
            Ok(json!({
                "entity": id.clone(),
                "name": r.get::<_, String>(1)?,
                "maker": r.get::<_, String>(2)?,
                "billions": billions,
                "read_from": r.get::<_, String>(4)?,
                "licence": r.get::<_, String>(5)?,
                "context": r.get::<_, i64>(6)?,
                "sellers": r.get::<_, i64>(7)?,
                "boards": r.get::<_, i64>(8)?,
                "in": r.get::<_, Option<i64>>(9)?,
                "out": r.get::<_, Option<i64>>(10)?,
                "band": band.map(|b| b.0).unwrap_or(""),
                "band_name": band.map(|b| b.1).unwrap_or(""),
                // What it takes to hold it: about 0.65 GB per billion at four
                // bits, and a gigabyte back for the context and the runtime.
                "gb": ((billions * 0.65 + 1.0) * 10.0).round() / 10.0,
            }))
        })?;
        let mut out: Vec<Value> = rows.collect::<std::result::Result<_, _>>()?;
        out.retain(|r| addr.contains_key(r["entity"].as_str().unwrap_or("")));
        for row in out.iter_mut() {
            let eid = row["entity"].as_str().unwrap_or("").to_string();
            if let Some(h) = addr.get(&eid) {
                row["href"] = json!(h);
            }
        }
        Ok(out)
    }

    /// Models served inside a Trusted Execution Environment — a confidential
    /// variant where the host cannot read the prompt or the weights in the
    /// clear. Sellers list them as their own model names, suffixed `-TEE`, so
    /// that is the signal; each is matched back to its plain sibling by name.
    pub fn tee_models(&self) -> Result<Vec<Value>> {
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();
        // The plain sibling a `-TEE` name points back to, so the page can link
        // "the confidential build of X" to X.
        let base_href: HashMap<String, String> = addr
            .values()
            .map(|(name, href)| (name.to_lowercase(), href.clone()))
            .collect();
        let mut q = self.conn.prepare(
            "SELECT e.id, e.name, COALESCE(p.name,''), \
                    COALESCE(json_extract(e.attrs,'$.context'),0), \
                    (SELECT COUNT(DISTINCT o.provider_id) FROM offerings o \
                      WHERE o.entity_id = e.id AND o.status = 'live'), \
                    (SELECT COUNT(DISTINCT b.suite) FROM benchmarks b \
                      WHERE b.entity_id = e.id), \
                    (SELECT MIN(pr.micros_per_unit) FROM current_prices pr \
                       JOIN offerings o2 ON o2.id = pr.offering_id \
                      WHERE o2.entity_id = e.id AND COALESCE(o2.variant,'') = '' \
                        AND o2.status = 'live' AND pr.dimension = 'mtok_in'), \
                    (SELECT MIN(pr.micros_per_unit) FROM current_prices pr \
                       JOIN offerings o2 ON o2.id = pr.offering_id \
                      WHERE o2.entity_id = e.id AND COALESCE(o2.variant,'') = '' \
                        AND o2.status = 'live' AND pr.dimension = 'mtok_out') \
               FROM entities e LEFT JOIN providers p ON p.id = e.maker \
              WHERE e.register = 'model' AND UPPER(e.name) LIKE '%-TEE' \
              ORDER BY (SELECT COUNT(DISTINCT o.provider_id) FROM offerings o \
                          WHERE o.entity_id = e.id AND o.status = 'live') DESC, \
                       e.name COLLATE NOCASE",
        )?;
        let rows = q.query_map([], |r| {
            let name: String = r.get(1)?;
            let base = name
                .strip_suffix("-TEE")
                .or_else(|| name.strip_suffix("-tee"))
                .unwrap_or(&name)
                .to_string();
            Ok(json!({
                "entity": r.get::<_, String>(0)?,
                "name": name,
                "base": base,
                "maker": r.get::<_, String>(2)?,
                "context": r.get::<_, i64>(3)?,
                "sellers": r.get::<_, i64>(4)?,
                "boards": r.get::<_, i64>(5)?,
                "in": r.get::<_, Option<i64>>(6)?,
                "out": r.get::<_, Option<i64>>(7)?,
            }))
        })?;
        let mut out: Vec<Value> = rows.collect::<std::result::Result<_, _>>()?;
        out.retain(|r| addr.contains_key(r["entity"].as_str().unwrap_or("")));
        for row in out.iter_mut() {
            let eid = row["entity"].as_str().unwrap_or("").to_string();
            if let Some((_, h)) = addr.get(&eid) {
                row["href"] = json!(h);
            }
            let base = row["base"].as_str().unwrap_or("").to_lowercase();
            if let Some(h) = base_href.get(&base) {
                row["base_href"] = json!(h);
            }
        }
        Ok(out)
    }

    pub fn free_page(&self) -> Result<Value> {
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();

        // Free to call: the seller's own price list says nought, on a lane
        // named for the fact. Nothing else counts — a nought anywhere else is
        // a rounding mistake, which is what the daily check is for.
        let mut q = self.conn.prepare(
            "SELECT o.entity_id, p.name, COALESCE(o.limits,''), e.register, \
                    COALESCE(o.allowance,'') \
               FROM offerings o \
               JOIN providers p ON p.id = o.provider_id \
               JOIN entities e ON e.id = o.entity_id \
              WHERE o.variant = 'free' \
              ORDER BY e.name",
        )?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .filter_map(|(eid, seller, limits, register, allowance)| {
                let (name, href) = addr.get(&eid)?;
                Some((eid, name.clone(), href.clone(), seller, limits, register, allowance))
            })
            .collect::<Vec<_>>();

        // One thing, one row. A model given away by both Groq and Cerebras is
        // not two entries — it is one entry with two people handing it out,
        // and printed twice it reads as a fault in the catalogue.
        let mut order: Vec<String> = Vec::new();
        let mut by: HashMap<String, Value> = HashMap::new();
        for (eid, name, href, seller, limits, register, allowance) in rows {
            let e = by.entry(eid.clone()).or_insert_with(|| {
                order.push(eid.clone());
                json!({"entity": eid, "name": name, "href": href,
                       "register": register, "from": []})
            });
            e["from"].as_array_mut().unwrap().push(json!({
                "seller": seller,
                "terms": if limits == "no end date given" { String::new() } else { limits },
                "allowance": serde_json::from_str::<Value>(&allowance).unwrap_or(Value::Null),
            }));
        }
        let called: Vec<Value> = order.into_iter().filter_map(|id| by.remove(&id)).collect();

        // Free to run: the licence lets you have the weights. The parameter
        // count is what says whether your machine can hold it, so it leads.
        let open = LICENCE_FAMILIES
            .iter()
            .find(|(k, _, _)| *k == "open")
            .map(|(_, _, w)| *w)
            .unwrap_or("1=0");
        let sql = format!(
            "SELECT id, json_extract(attrs,'$.params'), json_extract(attrs,'$.license') \
               FROM entities WHERE register='model' AND ({open}) \
              ORDER BY json_extract(attrs,'$.params') IS NULL, \
                       json_extract(attrs,'$.params') DESC, name"
        );
        let mut q = self.conn.prepare(&sql)?;
        let run: Vec<Value> = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<f64>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .filter_map(|(eid, params, licence)| {
                let (name, href) = addr.get(&eid)?;
                Some(json!({"entity": eid, "name": name, "href": href,
                            "params": params, "licence": licence}))
            })
            .collect();

        // Free until you sell what it makes.
        let mut q = self.conn.prepare(
            "SELECT id, json_extract(attrs,'$.license') FROM entities \
              WHERE json_extract(attrs,'$.license') LIKE 'cc-by-nc%' ORDER BY name",
        )?;
        let research: Vec<Value> = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
            .filter_map(|row| row.ok())
            .filter_map(|(eid, licence)| {
                let (name, href) = addr.get(&eid)?;
                Some(json!({"entity": eid, "name": name, "href": href, "licence": licence}))
            })
            .collect();

        Ok(json!({
            "kind": "free", "href": "/index/free",
            "called": called, "run": run, "research": research,
            "read": self.last_read()?,
        }))
    }

    pub fn list_page(&self, axes: &[(&str, &str)]) -> Result<Option<Value>> {
        let mut wheres: Vec<String> = Vec::new();
        let mut named: Vec<(String, String)> = Vec::new();
        for (axis, slug) in axes {
            match *axis {
                "for" => {
                    let tag = self
                        .task_tags()?
                        .into_iter()
                        .find(|t| address_slug(t) == *slug);
                    let Some(tag) = tag else { return Ok(None) };
                    wheres.push(format!(
                        "json_extract(attrs,'$.tasks') LIKE '%\"{}\"%'",
                        tag.replace('\'', "")
                    ));
                    named.push(("for".into(), tag));
                }
                "licence" => {
                    let Some(fam) = LICENCE_FAMILIES.iter().find(|(k, _, _)| *k == *slug) else {
                        return Ok(None);
                    };
                    wheres.push(fam.2.to_string());
                    named.push(("licence".into(), fam.0.to_string()));
                }
                "does" => {
                    let pair = self
                        .modality_pairs()?
                        .into_iter()
                        .find(|(i, o)| format!("{}-to-{}", address_slug(i), address_slug(o)) == *slug);
                    let Some((i, o)) = pair else { return Ok(None) };
                    wheres.push(format!(
                        "input_kind = '{}' AND output_kind = '{}'",
                        i.replace('\'', ""),
                        o.replace('\'', "")
                    ));
                    named.push(("does".into(), format!("{i} → {o}")));
                }
                "local" => {
                    let Some((_, gb, _)) = MEMORY_BANDS.iter().find(|(k, _, _)| *k == *slug) else {
                        return Ok(None);
                    };
                    wheres.push(format!(
                        "json_extract(attrs,'$.params') IS NOT NULL \
                         AND json_extract(attrs,'$.license') <> 'proprietary' \
                         AND json_extract(attrs,'$.params') <= {:.0}",
                        fits_billions(*gb) * 1e9
                    ));
                    named.push(("local".into(), slug.to_string()));
                }
                "register" => {
                    if !["model", "tool", "agent", "subscription"].contains(slug) {
                        return Ok(None);
                    }
                    wheres.push(format!("register = '{slug}'"));
                    named.push(("register".into(), slug.to_string()));
                }
                _ => return Ok(None),
            }
        }
        if wheres.is_empty() {
            return Ok(None);
        }
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();
        // When the question is what fits, the answer is the biggest that does,
        // so that one goes at the top rather than whatever starts with A.
        let order = if axes.iter().any(|(a, _)| *a == "local") {
            "json_extract(attrs,'$.params') DESC, name"
        } else {
            "name"
        };
        let sql = format!(
            "SELECT id, name, register, json_extract(attrs,'$.params') \
               FROM entities WHERE {} ORDER BY {order}",
            wheres.join(" AND ")
        );
        let mut q = self.conn.prepare(&sql)?;
        let members: Vec<Value> = q
            .query_map([], |r| {
                let id: String = r.get(0)?;
                let href = addr.get(&id).map(|(_, h)| h.clone()).unwrap_or_default();
                Ok(json!({"id": id, "name": r.get::<_, String>(1)?,
                          "register": r.get::<_, String>(2)?, "href": href,
                          // asked for in the same statement: a second query on
                          // a connection already stepping one quietly fails
                          "params": r.get::<_, Option<i64>>(3)?}))
            })?
            .collect::<std::result::Result<_, _>>()?;
        if members.is_empty() {
            return Ok(None);
        }
        let href = axes
            .iter()
            .fold("/index".to_string(), |acc, (a, v)| format!("{acc}/{a}/{v}"));
        Ok(Some(json!({
            "kind": "list", "axes": named.iter().map(|(a, v)| json!({"axis": a, "value": v}))
                .collect::<Vec<_>>(),
            "href": href, "members": members,
        })))
    }

    /// Every task word in use, so a list can be built for each.
    pub fn task_tags(&self) -> Result<Vec<String>> {
        let mut q = self.conn.prepare(
            "SELECT DISTINCT j.value FROM entities e, json_each(json_extract(e.attrs,'$.tasks')) j \
              ORDER BY 1",
        )?;
        let out = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// Every pair of modalities the catalogue actually sells, biggest first.
    pub fn modality_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut q = self.conn.prepare(
            "SELECT input_kind, output_kind, COUNT(*) c FROM entities \
              GROUP BY 1,2 ORDER BY c DESC",
        )?;
        let out = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// The freshest reading anywhere in the catalogue. Every page says it,
    /// because a price with no date behind it is a rumour.
    pub fn last_read(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row(
                "SELECT MAX(d) FROM (SELECT MAX(taken_at) d FROM prices \
                  UNION ALL SELECT MAX(taken_at) FROM benchmarks \
                  UNION ALL SELECT MAX(taken_at) FROM docs)",
                [],
                |r| r.get::<_, Option<String>>(0),
            )?
            .unwrap_or_default())
    }

    /// The span of readings behind one thing's prices: one date when the
    /// whole card was read at once, two when it was not.
    pub fn read_span(&self, entity_id: &str) -> Result<Option<(String, String)>> {
        let span: Option<(Option<String>, Option<String>)> = self
            .conn
            .query_row(
                "SELECT MIN(x.taken_at), MAX(x.taken_at) FROM prices x \
                   JOIN offerings o ON o.id = x.offering_id WHERE o.entity_id = ?1",
                params![entity_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match span {
            Some((Some(a), Some(b))) => Some((a, b)),
            _ => None,
        })
    }

    /// Every board with its name and how many of its ranked models the
    /// catalogue holds — one query, where the map of lists was making
    /// forty-two round trips to draw one page.
    pub fn board_counts(&self) -> Result<Vec<(String, String, i64)>> {
        let mut q = self.conn.prepare(
            "SELECT b.suite, COALESCE(s.name, b.suite), COUNT(DISTINCT b.entity_id) \
               FROM benchmarks b LEFT JOIN suites s ON s.id = b.suite \
              GROUP BY b.suite ORDER BY 3 DESC",
        )?;
        let out = q
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(out)
    }

    pub fn count_register(&self, register: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM entities WHERE register=?1",
            params![register],
            |r| r.get(0),
        )?)
    }

    pub fn count(&self, table: &str) -> Result<i64> {
        // The table name comes from our own call sites, never from input.
        Ok(self
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_between_is_the_real_calendar() {
        assert_eq!(days_between("2026-09-01", "2026-09-01"), 0);
        assert_eq!(days_between("2026-08-01", "2026-09-01"), 31);
        assert_eq!(days_between("2026-09-10", "2026-09-01"), 0);
        // The exact case the SQL view and this function must agree on: a true
        // 45-day gap. A 31-day-month shortcut read it as 48 and dropped a
        // seller the view kept.
        assert_eq!(days_between("2026-01-19", "2026-03-05"), 45);
        // Across a leap day, February included in full.
        assert_eq!(days_between("2024-02-01", "2024-03-01"), 29);
        assert_eq!(days_between("2026-02-01", "2026-03-01"), 28);
    }

    fn index() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let ix = Index::open(path.to_str().unwrap()).unwrap();
        (dir, ix)
    }

    fn model(ix: &Index, id: &str) {
        ix.insert_entity(&Entity {
            id: id.into(),
            register: "model".into(),
            name: id.into(),
            input_kind: "text".into(),
            output_kind: "text".into(),
            attrs: "{}".into(),
            ..Default::default()
        })
        .unwrap();
    }

    fn provider(ix: &Index, id: &str) {
        ix.upsert_provider(&Provider {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn one_model_five_providers_five_ways() {
        // pass-index stage 1: the data model's own acceptance check (spec §9).
        let (_d, ix) = index();
        model(&ix, "ent_m");
        let ways = ["api", "aggregator", "cloud", "local", "mcp"];
        for (i, way) in ways.iter().enumerate() {
            let p = format!("prov_{i}");
            provider(&ix, &p);
            let off = ix.upsert_offering("ent_m", &p, way, "", "2026-08-24").unwrap();
            ix.add_price(off, "mtok_in", 1_000_000 + i as i64, "https://src", "2026-08-24")
                .unwrap();
            ix.add_price(off, "mtok_out", 5_000_000 + i as i64, "https://src", "2026-08-24")
                .unwrap();
        }
        assert_eq!(ix.count("entities").unwrap(), 1);
        assert_eq!(ix.count("offerings").unwrap(), 5);
        assert_eq!(ix.count("prices").unwrap(), 10);
        assert_eq!(ix.providers().unwrap().len(), 5);
        let views = ix.offerings_of("ent_m").unwrap();
        assert_eq!(views.len(), 5);
        for v in &views {
            assert_eq!(v.components.len(), 2);
            assert!(v.components.iter().all(|c| c.basis == "declared"));
        }
        let out = ix.export_json().unwrap();
        assert_eq!(out["entities"].as_array().unwrap().len(), 1);
        assert_eq!(out["entities"][0]["offerings"].as_array().unwrap().len(), 5);
        assert_eq!(out["providers"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn a_new_source_name_is_one_alias_row() {
        // pass-index stage 1: spec §9 — a sixth source touches nothing else.
        let (_d, ix) = index();
        model(&ix, "ent_m");
        ix.bind_alias("prov_openrouter", "vendor/model-v5", "ent_m").unwrap();
        assert_eq!(ix.resolve("prov_openrouter", "vendor/model-v5").unwrap().as_deref(), Some("ent_m"));
        assert_eq!(ix.aliases_of("ent_m").unwrap(), vec![("prov_openrouter".into(), "vendor/model-v5".into())]);
        assert_eq!(ix.count("aliases").unwrap(), 1);
        assert_eq!(ix.count("entities").unwrap(), 1);
        assert_eq!(ix.count("offerings").unwrap(), 0);
    }

    #[test]
    fn voice_agent_bills_per_minute_plus_per_call() {
        // pass-index stage 1: spec §9 — two components, one offering, no schema change.
        let (_d, ix) = index();
        provider(&ix, "prov_v");
        ix.insert_entity(&Entity {
            id: "ent_voice".into(),
            register: "agent".into(),
            name: "voice agent".into(),
            input_kind: "goal".into(),
            output_kind: "call outcome".into(),
            attrs: "{}".into(),
            ..Default::default()
        })
        .unwrap();
        let off = ix.upsert_offering("ent_voice", "prov_v", "api", "", "2026-08-24").unwrap();
        ix.add_price(off, "minute", 50_000, "https://src", "2026-08-24").unwrap();
        ix.add_price(off, "call", 10_000, "https://src", "2026-08-24").unwrap();
        let comps = ix.current_price(off).unwrap();
        assert_eq!(comps.len(), 2);
        assert_eq!(ix.count("offerings").unwrap(), 1);
    }

    #[test]
    fn a_component_without_provenance_is_refused() {
        // pass-index stage 1: source and date are NOT NULL — no figure without them.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        let off = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-24").unwrap();
        let refused = ix.conn.execute(
            "INSERT INTO prices (offering_id, dimension, micros_per_unit, source_url, taken_at)
             VALUES (?1, 'mtok_in', 1, NULL, NULL)",
            params![off],
        );
        assert!(refused.is_err());
    }

    #[test]
    fn current_price_is_the_latest_and_history_stays() {
        // pass-index stage 1: append-only — a new component supersedes, never rewrites.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        let off = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-24").unwrap();
        ix.add_price(off, "mtok_in", 100, "https://old", "2026-08-01").unwrap();
        ix.add_price(off, "mtok_in", 90, "https://new", "2026-08-24").unwrap();
        let comps = ix.current_price(off).unwrap();
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].micros_per_unit, 90);
        assert_eq!(comps[0].source_url, "https://new");
        assert_eq!(ix.count("prices").unwrap(), 2);
    }

    #[test]
    fn a_repeat_sighting_is_the_same_offering() {
        // pass-index stage 1: one row per (entity, provider, way, variant).
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        let a = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-01").unwrap();
        let b = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-24").unwrap();
        assert_eq!(a, b);
        assert_eq!(ix.count("offerings").unwrap(), 1);
        let seen: String = ix
            .conn
            .query_row("SELECT last_seen FROM offerings WHERE id=?1", params![a], |r| r.get(0))
            .unwrap();
        assert_eq!(seen, "2026-08-24");
    }

    #[test]
    fn the_register_list_is_closed() {
        // pass-index stage 1: model | tool | agent — the schema refuses a fourth kind.
        let (_d, ix) = index();
        let refused = ix.insert_entity(&Entity {
            id: "ent_x".into(),
            register: "compute".into(),
            name: "x".into(),
            input_kind: "a".into(),
            output_kind: "b".into(),
            attrs: "{}".into(),
            ..Default::default()
        });
        assert!(refused.is_err());
    }

    #[test]
    fn a_priced_way_without_a_price_is_swept_but_local_stays() {
        // pass-index stage 2: an offering left behind by re-curation says
        // nothing; open weights on your own hardware legitimately say nothing.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        let api = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-25").unwrap();
        let local = ix.upsert_offering("ent_m", "prov_p", "local", "", "2026-08-25").unwrap();
        let priced = ix.upsert_offering("ent_m", "prov_p", "cloud", "", "2026-08-25").unwrap();
        ix.add_price(priced, "mtok_in", 1, "https://src", "2026-08-25").unwrap();
        assert_eq!(ix.drop_empty_offerings().unwrap(), 1);
        let left: Vec<i64> = ix.offerings_of("ent_m").unwrap().iter().map(|o| o.offering_id).collect();
        assert!(left.contains(&local) && left.contains(&priced) && !left.contains(&api));
    }

    #[test]
    fn a_published_score_hangs_on_the_entity_and_moves_only_when_it_moves() {
        // pass-index benchmarks: quality belongs to the weights, and a board
        // read twice unchanged leaves one row.
        let (_d, ix) = index();
        model(&ix, "ent_m");
        ix.upsert_suite("lmarena_text", "LMArena · Text", Some("LMArena"),
            Some("https://lmarena.ai/leaderboard"), Some("elo"), Some("model"), false,
            Some("2026-08-24")).unwrap();
        assert!(ix.add_benchmark_if_changed("ent_m", "lmarena_text", "elo", 1462.0,
            Some(1), Some(84), "https://lmarena.ai/leaderboard", "2026-08-25").unwrap());
        assert!(!ix.add_benchmark_if_changed("ent_m", "lmarena_text", "elo", 1462.0,
            Some(1), Some(84), "https://lmarena.ai/leaderboard", "2026-08-26").unwrap());
        // a slipped rank is news even when the score holds
        assert!(ix.add_benchmark_if_changed("ent_m", "lmarena_text", "elo", 1462.0,
            Some(3), Some(86), "https://lmarena.ai/leaderboard", "2026-08-27").unwrap());
        let rows = ix.benchmarks_of("ent_m").unwrap();
        assert_eq!(rows.len(), 1, "one standing per suite and metric");
        assert_eq!(rows[0]["rank"], 3);
        assert_eq!(rows[0]["basis"], "published");
        assert_eq!(rows[0]["measurer"], "LMArena");
        assert_eq!(ix.count("benchmarks").unwrap(), 2, "the earlier standing stays as history");
    }

    #[test]
    fn a_card_shows_the_best_config_of_the_current_reading() {
        // A board that lists one model in several configurations at once, all
        // read the same night under one metric label: the card must show the
        // best config, not whichever landed last.
        let (_d, ix) = index();
        model(&ix, "ent_m");
        ix.upsert_suite("terminal_bench", "Terminal-Bench", Some("TB"),
            Some("https://tb"), Some("acc"), Some("model"), false, Some("2026-09-01")).unwrap();
        // inserted worst-last, all the same day
        ix.add_benchmark_if_changed("ent_m","terminal_bench","Accuracy",78.4,
            Some(9),Some(142),"https://tb","2026-09-01").unwrap();
        ix.add_benchmark_if_changed("ent_m","terminal_bench","Accuracy",69.1,
            Some(25),Some(142),"https://tb","2026-09-01").unwrap();
        let rows = ix.benchmarks_of("ent_m").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["rank"], 9, "best config of the current reading, not the last inserted");
    }

    #[test]
    fn tee_page_lists_only_confidential_variants_and_links_the_plain_one() {
        // /index/tee: a model served in a Trusted Execution Environment is
        // listed by its own -TEE name, matched back to its plain sibling.
        let (_d, ix) = index();
        provider(&ix, "prov_ds");
        for (id, name) in [
            ("ent_ds", "DeepSeek-V3.2"),
            ("ent_ds_tee", "DeepSeek-V3.2-TEE"),
            ("ent_llama", "Llama-4"),
        ] {
            ix.insert_entity(&Entity {
                id: id.into(),
                register: "model".into(),
                name: name.into(),
                maker: Some("prov_ds".into()),
                input_kind: "text".into(),
                output_kind: "text".into(),
                attrs: "{}".into(),
                ..Default::default()
            })
            .unwrap();
        }
        let rows = ix.tee_models().unwrap();
        assert_eq!(rows.len(), 1, "only the -TEE variant is a TEE model");
        assert_eq!(rows[0]["name"], "DeepSeek-V3.2-TEE");
        assert_eq!(rows[0]["base"], "DeepSeek-V3.2");
        assert!(
            rows[0]["base_href"].as_str().is_some(),
            "the plain sibling exists, so the page can link to it"
        );
    }

    #[test]
    fn tied_benchmark_rows_come_out_in_a_stable_order() {
        // Two rows from one suite at the same rank but under different metrics
        // have an identical (rank, suite-name) sort key. Without a unique
        // tiebreak their order follows insertion, so a later insert flips the
        // pair and the public daily dump shows a diff with no data change. The
        // order must be by metric, not by which landed first.
        let (_d, ix) = index();
        model(&ix, "ent_m");
        ix.upsert_suite("swe", "SWE-bench", None, Some("https://swe"),
            Some("acc"), Some("model"), false, Some("2026-09-01")).unwrap();
        // inserted with the later-alphabetical metric first
        ix.add_benchmark_if_changed("ent_m","swe","Pass@1 (thinking)",70.0,
            Some(3),None,"https://swe","2026-09-01").unwrap();
        ix.add_benchmark_if_changed("ent_m","swe","Pass@1",65.0,
            Some(3),None,"https://swe","2026-09-01").unwrap();
        let rows = ix.benchmarks_of("ent_m").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["metric"], "Pass@1",
            "tied rows order by metric, not by insertion order");
        assert_eq!(rows[1]["metric"], "Pass@1 (thinking)");
    }

    #[test]
    fn source_text_keeps_its_source() {
        // pass-index about: the About block is written from read text, so the
        // text is stored with where it was read and nothing else.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        ix.upsert_doc("prov_p", "one_liner", None, "We build frontier models.",
            "https://example.com/about", "2026-08-25").unwrap();
        ix.upsert_doc("prov_p", "fact", Some("hq"), "San Francisco, USA",
            "https://example.com/about", "2026-08-25").unwrap();
        // the same page read again updates in place
        ix.upsert_doc("prov_p", "one_liner", None, "We build frontier AI models.",
            "https://example.com/about", "2026-08-26").unwrap();
        let docs = ix.docs_of("prov_p").unwrap();
        assert_eq!(docs.len(), 2);
        let one = docs.iter().find(|d| d["kind"] == "one_liner").unwrap();
        assert_eq!(one["text"], "We build frontier AI models.");
        assert_eq!(one["taken_at"], "2026-08-26");
    }

    #[test]
    fn merging_moves_everything_and_keeps_both_lanes() {
        // pass-index: one model minted twice under two sources' names folds
        // into one, and the colliding offering becomes its own lane rather
        // than overwriting the survivor's.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_keep");
        model(&ix, "ent_drop");
        let a = ix.upsert_offering("ent_keep", "prov_p", "api", "", "2026-08-25").unwrap();
        ix.add_price(a, "mtok_in", 100, "https://src", "2026-08-25").unwrap();
        let b = ix.upsert_offering("ent_drop", "prov_p", "api", "", "2026-08-25").unwrap();
        ix.add_price(b, "mtok_in", 140, "https://src", "2026-08-25").unwrap();
        ix.bind_alias("prov_p", "vendor/drop", "ent_drop").unwrap();
        ix.upsert_suite("s", "Suite", None, None, None, None, false, None).unwrap();
        ix.add_benchmark_if_changed("ent_drop", "s", "score", 9.0, Some(1), None,
            "https://src", "2026-08-25").unwrap();
        ix.upsert_doc("ent_drop", "description", None, "text", "https://src", "2026-08-25").unwrap();

        let (offs, aliases, standings, texts) =
            ix.merge_entity("ent_drop", "ent_keep", "preview").unwrap();
        assert_eq!((offs, aliases, standings, texts), (1, 1, 1, 1));
        assert!(ix.entities("model").unwrap().iter().all(|e| e.id != "ent_drop"));
        let views = ix.offerings_of("ent_keep").unwrap();
        assert_eq!(views.len(), 2);
        assert!(views.iter().any(|v| v.variant == "preview"));
        assert_eq!(ix.resolve("prov_p", "vendor/drop").unwrap().as_deref(), Some("ent_keep"));
        assert_eq!(ix.benchmarks_of("ent_keep").unwrap().len(), 1);
        assert_eq!(ix.docs_of("ent_keep").unwrap().len(), 1);
    }

    #[test]
    fn a_source_sentence_becomes_the_catalogue_vocabulary() {
        // pass-index stage 3: modality columns read the same for every entity.
        assert_eq!(normalise_kind("text"), "text");
        assert_eq!(normalise_kind("image + text"), "text + image");
        assert_eq!(normalise_kind("speech (audio) + text"), "text + audio");
        assert_eq!(normalise_kind("Text prompt."), "text");
        assert_eq!(normalise_kind("documents (PDF, PPTX, DOCX) + images (PNG, AVIF)"), "image + file");
        assert_eq!(normalise_kind("video (up to 4K resolution, 30 or 60 fps)"), "video");
        assert_eq!(normalise_kind("text (reasoning_content plus answer content)"), "text");
        assert_eq!(normalise_kind("embedding vector (\"supports user-defined output dimensions\")"), "embedding");
        // a bare "file" in prose is a WAV file, not a document input
        assert_eq!(
            normalise_kind("Synthesized speech audio; the API curl example writes a WAV file."),
            "audio"
        );
        assert_eq!(normalise_kind("text + file"), "text + file");
        // prose the vocabulary cannot read at all still leaves a usable word
        assert_eq!(normalise_kind("A grounded response with citations / grounding metadata."), "text");
        assert_eq!(normalise_kind(""), "");
    }

    #[test]
    fn facts_are_normalised_on_the_way_in() {
        // pass-index stage 3: what a source says is kept in docs, not in the column.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        ix.set_entity_facts("ent_m", Some("Text, with a voice chosen per request."), Some("audio (WAV)"), None)
            .unwrap();
        let e = ix.entities("model").unwrap().into_iter().find(|e| e.id == "ent_m").unwrap();
        assert_eq!(e.input_kind, "text");
        assert_eq!(e.output_kind, "audio");
        // saying the same thing a second way changes nothing
        assert!(!ix.set_entity_facts("ent_m", Some("text prompt"), Some("speech"), None).unwrap());
    }

    #[test]
    fn learning_the_size_of_the_field_is_news() {
        // pass-index stage 3: a rank means nothing without the field it ranks in.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        ix.upsert_suite("s", "Suite", None, None, None, Some("model"), false, None).unwrap();
        assert!(ix.add_benchmark_if_changed("ent_m", "s", "score", 9.0, Some(3), None,
                                            "https://board", "2026-08-25").unwrap());
        // the same standing said twice is not news
        assert!(!ix.add_benchmark_if_changed("ent_m", "s", "score", 9.0, Some(3), None,
                                             "https://board", "2026-08-25").unwrap());
        // the same standing with the field size now known is
        assert!(ix.add_benchmark_if_changed("ent_m", "s", "score", 9.0, Some(3), Some(183),
                                            "https://board", "2026-08-25").unwrap());
        let b = ix.benchmarks_of("ent_m").unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0]["out_of"], serde_json::json!(183));
    }

    #[test]
    fn metrics_carry_the_same_discipline() {
        // pass-index stage 1: speed figures — latest per metric, provenance required.
        let (_d, ix) = index();
        provider(&ix, "prov_p");
        model(&ix, "ent_m");
        let off = ix.upsert_offering("ent_m", "prov_p", "api", "", "2026-08-24").unwrap();
        ix.add_metric(off, "tokens_per_second", 80.0, "https://old", "2026-08-01").unwrap();
        ix.add_metric(off, "tokens_per_second", 95.0, "https://new", "2026-08-24").unwrap();
        let m = ix.current_metrics(off).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].value, 95.0);
        assert_eq!(m[0].basis, "declared");
    }
}

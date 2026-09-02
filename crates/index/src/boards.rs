//! Where a thing places, on boards other people run.
//!
//! The catalogue measures nothing itself. Every standing here was published
//! by somebody who ran the evaluation, and the row records who, where and
//! when — a rank without a field size is a boast, and a score without a
//! measurer is a rumour.
//!
//! Five readers, because the boards do not agree on a shape: one publishes
//! every evaluation as a single JSON file, one a leaderboard API, one a zip
//! of CSVs, several render the whole field as an HTML table and refuse a
//! request that does not look like a browser, and one rides along inside a
//! price feed the catalogue already reads every night.

use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// One placement as a board published it.
pub struct Placement {
    pub suite: String,
    pub board: String,
    pub metric: String,
    /// The name the board printed, before binding.
    pub name: String,
    pub value: f64,
    pub lower_is_better: bool,
    pub source_url: String,
    pub measurer: String,
    pub home: String,
}

/// Several leaderboards render the whole thing server-side and refuse a
/// request that does not look like a browser. The table is the data; there is
/// no API to find.
pub const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

// ---------------------------------------------------------------------------
// ARC Prize
// ---------------------------------------------------------------------------

/// ARC Prize publishes every evaluation of every model as one file.
pub fn arc_prize(doc: &serde_json::Value) -> Vec<Placement> {
    const SRC: &str = "https://arcprize.org/media/data/evaluations.json";
    const BOARDS: &[(&str, &str, &str)] = &[
        ("v1_Semi_Private", "arc_agi_1", "ARC-AGI-1 (Semi-Private)"),
        ("v2_Semi_Private", "arc_agi_2", "ARC-AGI-2 (Semi-Private)"),
        ("v3_Semi_Private", "arc_agi_3", "ARC-AGI-3 (Semi-Private)"),
        ("v1_Public_Eval", "arc_agi_1_public", "ARC-AGI-1 (Public Eval)"),
        ("v2_Public_Eval", "arc_agi_2_public", "ARC-AGI-2 (Public Eval)"),
    ];
    // The file is a list, or an object holding one.
    let items: Vec<&serde_json::Value> = match doc.as_array() {
        Some(a) => a.iter().collect(),
        None => doc
            .as_object()
            .and_then(|m| m.values().find(|v| v.is_array()))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
    };
    let mut out = Vec::new();
    for i in items {
        let Some((_, suite, board)) =
            BOARDS.iter().find(|(k, _, _)| Some(*k) == i["datasetId"].as_str())
        else {
            continue;
        };
        let Some(score) = i["score"].as_f64() else { continue };
        // displayLabel is sometimes a boolean flag rather than a label; the
        // id is the field that is always a name.
        let name = match i["displayLabel"].as_str() {
            Some(s) if !s.trim().is_empty() => s,
            _ => match i["modelId"].as_str() {
                Some(s) => s,
                None => continue,
            },
        };
        let (_, effort) = crate::resolve::strip_lanes(name);
        out.push(Placement {
            suite: suite.to_string(),
            board: board.to_string(),
            metric: format!("Score (%){}", effort.map(|e| format!(" ({e})")).unwrap_or_default()),
            name: name.to_string(),
            value: round_to(score * 100.0, 2),
            lower_is_better: false,
            source_url: SRC.into(),
            measurer: "ARC Prize Foundation".into(),
            home: "https://arcprize.org/leaderboard".into(),
        });
    }
    out
}

pub const ARC_PRIZE: &str = "https://arcprize.org/media/data/evaluations.json";

// ---------------------------------------------------------------------------
// TTS Arena
// ---------------------------------------------------------------------------

pub const TTS_ARENA: &str = "https://tts-agi-tts-arena-v2.hf.space/api/leaderboard";

pub fn tts_arena(doc: &serde_json::Value) -> Vec<Placement> {
    let rows = if doc["rows"].is_array() { &doc["rows"] } else { doc };
    let mut out = Vec::new();
    for i in rows.as_array().into_iter().flatten() {
        if !i.is_object() || i["suspended"].as_bool().unwrap_or(false) {
            continue;
        }
        let Some(elo) = i["elo"].as_f64() else { continue };
        out.push(Placement {
            suite: "tts_arena_v2".into(),
            board: "TTS Arena V2".into(),
            metric: "Elo".into(),
            name: i["name"].as_str().unwrap_or("").to_string(),
            value: elo,
            lower_is_better: false,
            source_url: TTS_ARENA.into(),
            measurer: "TTS-AGI".into(),
            home: "https://huggingface.co/spaces/TTS-AGI/TTS-Arena-V2".into(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Epoch AI
// ---------------------------------------------------------------------------

pub const EPOCH_ZIP: &str = "https://epoch.ai/data/benchmark_data.zip";

/// Four of Epoch's files are boards the catalogue already carries — its GPQA
/// and FrontierMath figures were always Epoch's — and those keep their
/// existing ids so their history stays continuous. The rest are boards in
/// their own right, named as Epoch's, because a replication is not the
/// original.
const EPOCH_KNOWN: &[(&str, &str, &str)] = &[
    ("gpqa_diamond.csv", "gpqa_diamond", "GPQA Diamond"),
    ("otis_mock_aime_2024_2025.csv", "epoch_otis_mock_aime", "OTIS Mock AIME 2024-2025"),
    ("frontiermath.csv", "epoch_frontiermath", "FrontierMath (Tiers 1-3)"),
    ("hle.csv", "epoch_hle", "Humanity's Last Exam (Epoch AI replication)"),
    ("hle_external.csv", "epoch_hle", "Humanity's Last Exam (Epoch AI replication)"),
];

/// The first of these a file carries is the figure its board is ranked on.
/// Twenty-three files in the same zip were skipped only because they name
/// their result something else.
const EPOCH_SCORE: &[&str] = &[
    "mean_score", "ECI Score", "Score", "Accuracy", "Overall score", "Average score",
    "Arena Score", "Pass@1", "Pass@1 score", "Score (AVG@5)", "Main score",
    "Pooled score", "Accuracy mean", "average_score", "% Score", "Binary accuracy",
    "Mean score", "Challenge score", "EM", "Overall accuracy", "Overall", "Average",
    "Average (%)", "Global average", "Percent correct", "Performance", "Correct",
    "Overall pass (%)", "Unguided % Solved", "Mean capability", "Average progress",
    "Win Rate (%)", "Score OPT@1", "GDP.pdf score", "120k token score",
    "Overall (no subtitles)",
];

fn epoch_pretty(stem: &str) -> String {
    let words = stem.replace("_external", "").replace('_', " ");
    let words = words.trim();
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str() + " — Epoch AI",
        None => " — Epoch AI".into(),
    }
}

/// Ten megabytes of zip, each CSV the whole field rather than a top slice.
pub fn epoch(zip_bytes: &[u8]) -> Result<Vec<Placement>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    let mut names: Vec<String> = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();
    names.sort();
    let mut out = Vec::new();
    for path in names {
        if !path.ends_with(".csv") || path.contains('/') {
            continue;
        }
        let mut body = String::new();
        {
            use std::io::Read;
            let mut f = z.by_name(&path)?;
            let mut raw = Vec::new();
            f.read_to_end(&mut raw)?;
            body = String::from_utf8_lossy(&raw).into_owned();
        }
        let mut rdr = csv::ReaderBuilder::new()
            .flexible(true)
            .from_reader(body.as_bytes());
        let cols: Vec<String> = match rdr.headers() {
            Ok(h) => h.iter().map(str::to_string).collect(),
            Err(_) => continue,
        };
        let rows: Vec<csv::StringRecord> = rdr.records().flatten().collect();
        if rows.len() < 8 {
            continue;
        }
        let Some(score_col) = EPOCH_SCORE.iter().find(|c| cols.iter().any(|x| x == *c)) else {
            continue;
        };
        // "Model version" where a file has one, "Name" where it does not.
        // Half of these files use the second and were skipped for it.
        let Some(name_col) = ["Model version", "Model", "System", "Name"]
            .iter()
            .find(|c| cols.iter().any(|x| x == *c))
        else {
            continue;
        };
        let si = cols.iter().position(|c| c == *score_col).unwrap();
        let ni = cols.iter().position(|c| c == *name_col).unwrap();
        let stem = path.trim_end_matches(".csv").replace("_external", "");
        let (suite, board) = match EPOCH_KNOWN.iter().find(|(f, _, _)| *f == path) {
            Some((_, s, n)) => (s.to_string(), n.to_string()),
            None => (
                if stem.starts_with("epoch") { stem.clone() } else { format!("epoch_{stem}") },
                epoch_pretty(&stem),
            ),
        };
        for row in &rows {
            let raw = row.get(ni).unwrap_or("").trim().to_string();
            let Some(mut v) = row.get(si).and_then(|s| s.parse::<f64>().ok()) else {
                continue;
            };
            if raw.is_empty() {
                continue;
            }
            // Epoch stores a proportion where a board would print a percentage.
            if (0.0..=1.0).contains(&v)
                && matches!(*score_col, "mean_score" | "Score" | "Accuracy")
            {
                v *= 100.0;
            }
            let (_, effort) = crate::resolve::strip_lanes(&raw);
            out.push(Placement {
                suite: suite.clone(),
                board: board.clone(),
                metric: format!(
                    "{score_col}{}",
                    effort.map(|e| format!(" ({e})")).unwrap_or_default()
                ),
                name: raw,
                value: round_to(v, 3),
                lower_is_better: false,
                source_url: EPOCH_ZIP.into(),
                measurer: "Epoch AI".into(),
                home: "https://epoch.ai/data/ai-benchmarking-dashboard".into(),
            });
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Boards that ship their field as an HTML table
// ---------------------------------------------------------------------------

/// (suite, name, url, model column, score column, metric, lower is better,
///  measurer, the board's home page)
pub const TABLES: &[(&str, &str, &str, &str, &str, &str, bool, &str, &str)] = &[
    ("lmarena_text", "LMArena · Text", "https://arena.ai/leaderboard/text",
     "Model", "Score", "Score (Elo)", false, "LMArena", "https://arena.ai/leaderboard/text"),
    ("lmarena_vision", "LMArena · Vision", "https://arena.ai/leaderboard/vision",
     "Model", "Score", "Score (Elo)", false, "LMArena", "https://arena.ai/leaderboard/vision"),
    ("lmarena_webdev", "LMArena · WebDev", "https://arena.ai/leaderboard/code",
     "Model", "Score", "Score (Elo)", false, "LMArena", "https://arena.ai/leaderboard/code"),
    ("lmarena_agent", "LMArena · Agent", "https://arena.ai/leaderboard/agent",
     "Model", "Net Improvement", "Net Improvement (%)", false, "LMArena",
     "https://arena.ai/leaderboard/agent"),
    ("lmarena_search", "LMArena · Search", "https://arena.ai/leaderboard/search",
     "Model", "Score", "Score (Elo)", false, "LMArena", "https://arena.ai/leaderboard/search"),
    ("aa_intelligence", "Artificial Analysis · Intelligence Index",
     "https://artificialanalysis.ai/leaderboards/models",
     "Model", "Artificial Analysis Inte", "Artificial Analysis Intelligence Index", false,
     "Artificial Analysis", "https://artificialanalysis.ai/leaderboards/models"),
    ("aider_polyglot", "Aider polyglot coding leaderboard",
     "https://aider.chat/docs/leaderboards/",
     "Model", "Percent correct", "Percent correct", false,
     "Aider", "https://aider.chat/docs/leaderboards/"),
    ("swe_rebench", "SWE-rebench Leaderboard", "https://swe-rebench.com/leaderboard",
     "Model", "Resolved Rate", "Resolved Rate (%)", false,
     "SWE-rebench (Nebius)", "https://swe-rebench.com/leaderboard"),
];

struct TablePats {
    svg: Regex,
    tag: Regex,
    space: Regex,
    cell: Regex,
    table: Regex,
    tr: Regex,
    caption: Regex,
    role: Regex,
    bracket: Regex,
    twospace: Regex,
    number: Regex,
}

fn tp() -> &'static TablePats {
    static P: OnceLock<TablePats> = OnceLock::new();
    P.get_or_init(|| TablePats {
        svg: Regex::new(r"(?s)<svg.*?</svg>").unwrap(),
        tag: Regex::new(r"<[^>]+>").unwrap(),
        space: Regex::new(r"\s+").unwrap(),
        cell: Regex::new(r"(?s)<t[dh][^>]*>(.*?)</t[dh]>").unwrap(),
        table: Regex::new(r"(?s)<table.*?</table>").unwrap(),
        tr: Regex::new(r"(?s)<tr[^>]*>.*?</tr>").unwrap(),
        caption: Regex::new(r"\s+[^\s·]+(?:\s+[^\s·]+)?\s+·\s+\S+\s*$").unwrap(),
        role: Regex::new(r"\s+(Model|Agent|System)\s*$").unwrap(),
        bracket: Regex::new(r"\s*\[([^\]]+)\]\s*").unwrap(),
        twospace: Regex::new(r"\s{2,}").unwrap(),
        number: Regex::new(r"-?\d[\d,]*\.?\d*").unwrap(),
    })
}

/// An icon carries a `<title>` — the maker's name — which lands in the text
/// of the cell in front of the model's own name. Drop the artwork first.
fn cells(tr: &str) -> Vec<String> {
    let p = tp();
    let clean = p.svg.replace_all(tr, " ");
    p.cell
        .captures_iter(&clean)
        .map(|c| {
            let text = p.tag.replace_all(c.get(1).unwrap().as_str(), " ");
            p.space.replace_all(&text, " ").trim().to_string()
        })
        .collect()
}

/// The model's name, without the subtitle a table sets under it.
///
/// One board writes the cell as "claude-fable-5 Anthropic · Proprietary": the
/// name, then the maker and the licence as a caption. The caption is data the
/// catalogue already holds, and left in place it stops every name in the
/// table from binding.
fn clean_name(cell: &str) -> String {
    let p = tp();
    let s = p.caption.replace(cell, "");
    let s = p.role.replace(&s, "");
    let s = p.bracket.replace_all(&s, " ($1) ");
    let s = p.twospace.replace_all(s.trim(), " ").trim().to_string();
    if s.is_empty() { cell.to_string() } else { s }
}

/// The first number in a cell. "1508 ±5" is 1508; "$1.80" is 1.80.
fn number(text: &str) -> Option<f64> {
    let cleaned = text.replace(',', "");
    tp().number
        .find(&cleaned)
        .and_then(|m| m.as_str().parse::<f64>().ok())
}

/// (name, score) from the widest table on the page, by column heading.
pub fn table_rows(html: &str, model_col: &str, score_col: &str) -> Vec<(String, f64)> {
    let p = tp();
    let Some(best) = p.table.find_iter(html).map(|m| m.as_str()).max_by_key(|t| t.len())
    else {
        return vec![];
    };
    let trs: Vec<&str> = p.tr.find_iter(best).map(|m| m.as_str()).collect();
    let mut head: Option<Vec<String>> = None;
    for tr in trs.iter().take(4) {
        let c = cells(tr);
        let has_model = c.iter().any(|x| x.eq_ignore_ascii_case(model_col));
        let has_score = c
            .iter()
            .any(|x| x.to_lowercase().contains(&score_col.to_lowercase()));
        if has_model && has_score {
            head = Some(c);
            break;
        }
    }
    let Some(head) = head else { return vec![] };
    let mi = head.iter().position(|x| x.eq_ignore_ascii_case(model_col)).unwrap();
    let si = head
        .iter()
        .position(|x| x.to_lowercase().contains(&score_col.to_lowercase()))
        .unwrap();
    let mut out = Vec::new();
    for tr in &trs {
        let c = cells(tr);
        if c.len() <= mi.max(si) || c == head {
            continue;
        }
        let Some(v) = number(&c[si]) else { continue };
        let nm = clean_name(&c[mi]);
        if nm.is_empty() || nm.eq_ignore_ascii_case(model_col) {
            continue;
        }
        out.push((nm, v));
    }
    out
}

// ---------------------------------------------------------------------------
// Standings riding along in a price feed
// ---------------------------------------------------------------------------

/// One seller's model list carries a `benchmarks` block on more than half its
/// models. Artificial Analysis's intelligence index is deliberately not taken
/// from here: it is read from Artificial Analysis directly, where the field is
/// larger, and one board with one source is the rule that keeps a standing
/// from appearing twice under two labels.
pub fn openrouter_benchmarks(doc: &serde_json::Value) -> Vec<Placement> {
    let src = crate::feed::OPENROUTER_MODELS;
    let non = Regex::new(r"[^a-z0-9]+").unwrap();
    let mut out = Vec::new();
    for m in doc["data"].as_array().into_iter().flatten() {
        let name = m["name"]
            .as_str()
            .or_else(|| m["id"].as_str())
            .unwrap_or("")
            .to_string();
        let aa = &m["benchmarks"]["artificial_analysis"];
        for (key, suite, title) in [
            ("coding_index", "aa_coding_index", "Artificial Analysis · Coding Index"),
            ("agentic_index", "aa_agentic_index", "Artificial Analysis · Agentic Index"),
        ] {
            if let Some(v) = aa[key].as_f64() {
                out.push(Placement {
                    suite: suite.into(),
                    board: title.into(),
                    metric: "Index".into(),
                    name: name.clone(),
                    value: v,
                    lower_is_better: false,
                    source_url: src.into(),
                    measurer: "Artificial Analysis".into(),
                    home: "https://artificialanalysis.ai".into(),
                });
            }
        }
        for row in m["benchmarks"]["design_arena"].as_array().into_iter().flatten() {
            let Some(elo) = row["elo"].as_f64() else { continue };
            let cat = row["category"].as_str().unwrap_or("").trim().to_string();
            let arena = row["arena"].as_str().unwrap_or("").trim().to_string();
            if cat.is_empty() {
                continue;
            }
            out.push(Placement {
                suite: format!(
                    "design_arena_{}",
                    non.replace_all(&cat.to_lowercase(), "_").trim_matches('_')
                ),
                board: format!(
                    "Design Arena · {cat}{}",
                    if !arena.is_empty() && arena != cat { format!(" ({arena})") } else { String::new() }
                ),
                metric: "Elo".into(),
                name: name.clone(),
                value: elo,
                lower_is_better: false,
                source_url: src.into(),
                measurer: "Design Arena".into(),
                home: "https://designarena.ai".into(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Ranking, and what survives it
// ---------------------------------------------------------------------------

/// A board often reports the same score for several reasoning efforts. That
/// is one result, not several: recording both would put a model on a board
/// twice with identical numbers, which reads as a bug however true it is.
/// The cheapest effort that reached the score is the one kept, because that
/// is the one worth buying.
const EFFORT_RANK: &[&str] = &[
    "minimal", "none", "low", "medium", "high", "xhigh", "max", "thinking", "reasoning",
];

pub struct Bound {
    pub entity: String,
    pub suite: String,
    pub board: String,
    pub metric: String,
    pub value: f64,
    pub rank: usize,
    pub out_of: usize,
    pub source_url: String,
    pub measurer: String,
    pub home: String,
}

/// Rank inside each board over the whole published field, so a placement says
/// how many it actually beat — not how many we matched.
pub fn rank_and_bind(
    rows: &[Placement],
    r: &mut crate::resolve::Resolver,
) -> (Vec<Bound>, Vec<(String, usize)>, usize) {
    let mut order: Vec<String> = Vec::new();
    let mut fields: HashMap<String, Vec<&Placement>> = HashMap::new();
    for p in rows {
        fields
            .entry(p.suite.clone())
            .or_insert_with(|| {
                order.push(p.suite.clone());
                Vec::new()
            })
            .push(p);
    }
    let boards = order.len();

    let mut bound: Vec<Bound> = Vec::new();
    let mut unbound: Vec<(String, usize)> = Vec::new();
    for suite in &order {
        let field = fields.get_mut(suite).unwrap();
        // Stable, so equal scores keep the order the board printed them in.
        //
        // Which way to sort comes from the LAST placement read, not from the
        // board being sorted. That is what the Python does — a loop variable
        // left bound after the loop that filled the buckets — and every board
        // read today says higher is better, so it has never differed. Copied
        // rather than corrected: the port is accepted by agreeing with the
        // Python, and this is a rule to change deliberately, not in passing.
        let lower = rows.last().is_some_and(|p| p.lower_is_better);
        if lower {
            field.sort_by(|a, b| a.value.total_cmp(&b.value));
        } else {
            field.sort_by(|a, b| b.value.total_cmp(&a.value));
        }
        let n = field.len();
        for (i, p) in field.iter().enumerate() {
            match r.bind(&p.name) {
                Some(eid) => bound.push(Bound {
                    entity: eid,
                    suite: p.suite.clone(),
                    board: p.board.clone(),
                    metric: p.metric.clone(),
                    value: p.value,
                    rank: i + 1,
                    out_of: n,
                    source_url: p.source_url.clone(),
                    measurer: p.measurer.clone(),
                    home: p.home.clone(),
                }),
                None => match unbound.iter_mut().find(|(nm, _)| *nm == p.name) {
                    Some(u) => u.1 += 1,
                    None => unbound.push((p.name.clone(), 1)),
                },
            }
        }
    }

    // One standing per model per board. The board ranks configurations —
    // effort levels, context budgets, agent scaffolds — and several of them
    // bind to one catalogue row. Keeping a row per distinct score meant a
    // model held ten standings on one board and the card printed whichever
    // came back last, which was the worst of them. The model's standing on a
    // board is its best configuration's; among equal ranks the cheaper
    // effort wins, because that is the one a buyer would run.
    let effort_re = Regex::new(r"\(([^)]*)\)\s*$").unwrap();
    let mut keep: Vec<(String, String, usize, Bound)> = Vec::new();
    for b in bound {
        let effort = effort_re
            .captures(&b.metric)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().split(',').next_back().unwrap_or("").trim().to_lowercase());
        let cost = effort
            .as_deref()
            .and_then(|e| EFFORT_RANK.iter().position(|x| *x == e))
            .unwrap_or(99);
        match keep
            .iter_mut()
            .find(|(e, s, _, _)| *e == b.entity && *s == b.suite)
        {
            Some(slot) => {
                let better = b.rank < slot.3.rank || (b.rank == slot.3.rank && cost < slot.2);
                if better {
                    *slot = (b.entity.clone(), b.suite.clone(), cost, b);
                }
            }
            None => keep.push((b.entity.clone(), b.suite.clone(), cost, b)),
        }
    }
    (keep.into_iter().map(|k| k.3).collect(), unbound, boards)
}

/// This reader is the authority for the boards it reads. An earlier crawl of
/// the same board under a different metric label would otherwise sit beside
/// the new row as a second standing for one result, so the board's old rows
/// go before the new ones land.
pub fn write_standings(
    con: &rusqlite::Connection,
    bound: &[Bound],
    today: &str,
) -> Result<usize> {
    let mut suites: Vec<(&str, &str, &str, &str)> = Vec::new();
    for b in bound {
        let row = (b.suite.as_str(), b.board.as_str(), b.measurer.as_str(), b.home.as_str());
        if !suites.contains(&row) {
            suites.push(row);
        }
    }
    for (id, name, measurer, url) in &suites {
        con.execute(
            "INSERT OR IGNORE INTO suites (id,name,measurer,url,subject,lower_is_better) \
             VALUES (?1,?2,?3,?4,'model',0)",
            rusqlite::params![id, name, measurer, url],
        )?;
    }
    let mut cleared: Vec<&str> = Vec::new();
    for b in bound {
        if !cleared.contains(&b.suite.as_str()) {
            con.execute("DELETE FROM benchmarks WHERE suite=?1", [&b.suite])?;
            cleared.push(&b.suite);
        }
    }
    for b in bound {
        con.execute(
            "INSERT INTO benchmarks (entity_id,suite,metric,value,rank,out_of,basis,\
             source_url,taken_at) VALUES (?1,?2,?3,?4,?5,?6,'published',?7,?8)",
            rusqlite::params![
                b.entity, b.suite, b.metric, b.value, b.rank as i64, b.out_of as i64,
                b.source_url, today
            ],
        )?;
    }
    Ok(bound.len())
}

/// Rounding, the way Python rounds.
///
/// Multiplying by a power of ten, rounding and dividing back accumulates the
/// error of the binary value and lands on the other side of the boundary
/// often enough to matter: one score read 0.275 here and 0.274 there, and the
/// same arithmetic is used as a dedup key, so a whole placement appeared on
/// one side and not the other. Formatting to the wanted precision rounds the
/// decimal representation with ties to even, which is what Python's `round`
/// does.
fn round_to(v: f64, places: usize) -> f64 {
    format!("{v:.places$}").parse().unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caption_under_a_name_is_not_part_of_it() {
        assert_eq!(clean_name("claude-fable-5 Anthropic · Proprietary"), "claude-fable-5");
        assert_eq!(clean_name("Fable 5 [high] Model"), "Fable 5 (high)");
        assert_eq!(clean_name("Junie Agent"), "Junie");
    }

    #[test]
    fn the_first_number_in_a_cell_is_the_score() {
        assert_eq!(number("1508 ±5"), Some(1508.0));
        assert_eq!(number("$1.80"), Some(1.80));
        assert_eq!(number("1,234.5"), Some(1234.5));
        assert_eq!(number("—"), None);
    }

    /// Half to even, because the Python this replaces rounds that way and a
    /// score ending in a five must land on the same side.
    #[test]
    fn rounding_matches_the_python() {
        assert_eq!(round_to(2.5, 0), 2.0);
        assert_eq!(round_to(3.5, 0), 4.0);
        assert_eq!(round_to(0.125, 2), 0.12);
        // Written as 0.274 when this test was born — and never run. The
        // double nearest 0.2745 sits above it, so Python itself says 0.275,
        // and the acceptance runs that compared standings byte-for-byte
        // already proved the two agree.
        assert_eq!(round_to(0.2745, 3), 0.275);
    }
}

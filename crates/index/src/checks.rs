//! Consistency checks for the catalogue.
//!
//! The catalogue is assembled by crawlers, and a crawler is a machine that
//! writes plausible things. Every defect checked for here is one that was
//! actually found by hand, in a catalogue that looked fine: one model
//! arriving twice from two feeds under two names; a rate that rounded below
//! the smallest unit the catalogue stores, so a real price read "$0" and no
//! later crawl could correct it; a resale recorded as if the seller made the
//! thing; a maker named on a product nobody makes.
//!
//! Each check states what it looked for and what it found, prints a few
//! examples, and says whether the finding blocks. A blocking finding means
//! the catalogue is asserting something false to a reader; a warning means it
//! is merely incomplete.
//!
//! The 26 checks that are one query were lifted out of the Python by a script
//! rather than retyped, so the two cannot drift by a character.

use anyhow::Result;
use regex::Regex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Rows the catalogue holds on purpose and must not be nagged about: a
/// capability the whole market sells is nobody's product.
const COMMONS: &[&str] = &["ent_web-search", "ent_code-execution"];
const KINDS: &[&str] = &["text", "code", "image", "audio", "video", "file", "embedding"];
const PROVIDER_KINDS: &[&str] = &["vendor", "aggregator", "cloud", "fund"];
const WAYS: &[&str] = &["api", "aggregator", "cloud", "local", "subscription", "mcp"];
const REGISTERS: &[&str] = &["model", "tool", "agent", "subscription"];

pub struct Check {
    pub name: &'static str,
    pub blocking: bool,
    pub question: &'static str,
    pub sql: &'static str,
}

/// One finding: the two columns a check hands back for a reader to look at.
pub type Finding = (String, String);

pub const QUERIES: &[Check] = &[
    Check {
        name: "one model string, two entities",
        blocking: true,
        question: "a seller's own id for a model cannot name two different models",
        sql: "WITH shared AS (SELECT alias FROM aliases\n                        WHERE alias LIKE '%/%' AND alias NOT LIKE '% %'\n                        GROUP BY alias HAVING COUNT(DISTINCT entity_id) > 1)\n        SELECT s.alias, GROUP_CONCAT(a.entity_id, ' + ')\n        FROM aliases a JOIN shared s ON s.alias = a.alias GROUP BY s.alias",
    },
    Check {
        name: "one name, two entities under one maker",
        blocking: true,
        question: "two cards from the same maker reading the same are one card twice",
        sql: "SELECT name, GROUP_CONCAT(id, ' + ') FROM entities\n        GROUP BY maker, LOWER(name) HAVING COUNT(*) > 1",
    },
    Check {
        name: "a row that contradicts itself about the weights",
        blocking: true,
        question: "a thing cannot have published weights and a proprietary licence; one of the two collectors that wrote this row read its source wrong",
        sql: "SELECT name, json_extract(attrs,'$.license') FROM entities\n         WHERE json_extract(attrs,'$.license') = 'proprietary'\n           AND json_extract(attrs,'$.open_weights') = 1",
    },
    Check {
        name: "a thing whose maker is gone",
        blocking: true,
        question: "an entity pointing at a provider row that no longer exists — a fold or a sweep took the company and left the models behind",
        sql: "SELECT e.name, e.maker FROM entities e\n         WHERE e.maker IS NOT NULL\n           AND NOT EXISTS(SELECT 1 FROM providers p WHERE p.id = e.maker)",
    },
    Check {
        name: "two descriptions for one thing",
        blocking: true,
        question: "a card can print one, so the second is a coin toss between two sources; the maker's own words win and the rest are dropped when they are read",
        sql: "SELECT COALESCE(p.name, e.name), COUNT(*) FROM docs d\n          LEFT JOIN providers p ON p.id = d.subject\n          LEFT JOIN entities e ON e.id = d.subject\n         WHERE d.kind = 'description'\n         GROUP BY d.subject HAVING COUNT(*) > 1",
    },
    Check {
        name: "a description that says nothing",
        blocking: true,
        question: "under forty characters is a title, a placeholder or a blank, and it reads on the page as though nobody bothered",
        sql: "SELECT COALESCE(p.name, e.name), LENGTH(TRIM(d.text)) FROM docs d\n          LEFT JOIN providers p ON p.id = d.subject\n          LEFT JOIN entities e ON e.id = d.subject\n         WHERE d.kind = 'description' AND LENGTH(TRIM(d.text)) < 40",
    },
    Check {
        name: "published before its own training data",
        blocking: true,
        question: "a model cannot come out before the material it was trained on; the earliest date any seller gave was being taken as the release date",
        sql: "SELECT name, json_extract(attrs,'$.released') || ' but trained to '\n                  || json_extract(attrs,'$.knowledge')\n          FROM entities\n         WHERE json_extract(attrs,'$.released') IS NOT NULL\n           AND json_extract(attrs,'$.knowledge') IS NOT NULL\n           AND json_extract(attrs,'$.released') < json_extract(attrs,'$.knowledge')",
    },
    Check {
        name: "a seller credited as the maker",
        blocking: true,
        question: "a gateway did not build the model it resells; the feed's path put the seller's name where the maker's belongs",
        sql: "SELECT e.name, p.name FROM entities e JOIN providers p ON p.id = e.maker\n         WHERE p.kind IN ('aggregator','cloud')\n           AND NOT EXISTS(SELECT 1 FROM offerings o JOIN entities x ON x.id = o.entity_id\n                           WHERE o.provider_id = p.id AND x.maker = p.id\n                             AND o.way = 'api')",
    },
    Check {
        name: "one lane written twice at one price",
        blocking: false,
        question: "two readings of a seller's page leave a row with no lane and a row named after the tier, at the identical rate",
        sql: "SELECT e.name || ' at ' || p.name, COUNT(*) FROM offerings a\n          JOIN offerings b ON b.entity_id=a.entity_id AND b.provider_id=a.provider_id\n                          AND b.way=a.way AND b.id>a.id\n          JOIN entities e ON e.id=a.entity_id\n          JOIN providers p ON p.id=a.provider_id\n         WHERE COALESCE(a.variant,'')=''\n           AND (SELECT GROUP_CONCAT(dimension||':'||micros_per_unit)\n                  FROM prices WHERE offering_id=a.id)\n             = (SELECT GROUP_CONCAT(dimension||':'||micros_per_unit)\n                  FROM prices WHERE offering_id=b.id)\n         GROUP BY a.id",
    },
    Check {
        name: "one name, two providers",
        blocking: true,
        question: "a company filed twice sells its catalogue twice",
        sql: "SELECT name, GROUP_CONCAT(id, ' + ') FROM providers\n        GROUP BY LOWER(name) HAVING COUNT(*) > 1",
    },
    Check {
        name: "one standing under two labels",
        blocking: true,
        question: "the same figure on the same board, recorded twice because two crawls named the metric differently",
        sql: "WITH latest AS (SELECT b.* FROM benchmarks b JOIN\n            (SELECT entity_id, suite, metric, MAX(id) mid FROM benchmarks GROUP BY 1,2,3) l\n            ON b.id = l.mid)\n        SELECT entity_id || ' on ' || suite, GROUP_CONCAT(metric, ' | ')\n        FROM latest GROUP BY entity_id, suite, ROUND(value, 1) HAVING COUNT(*) > 1",
    },
    Check {
        name: "a price of nought",
        blocking: true,
        question: "a rate that rounded below a micro-dollar is not free, and a stored nought is never corrected because the next crawl skips the dimension — unless the seller declared it free, which is the `free` lane",
        sql: "SELECT e.name || ' at ' || p.name, x.dimension\n        FROM prices x JOIN offerings o ON o.id = x.offering_id\n        JOIN entities e ON e.id = o.entity_id JOIN providers p ON p.id = o.provider_id\n        WHERE x.micros_per_unit = 0 AND COALESCE(o.variant,'') <> 'free'",
    },
    Check {
        name: "a resale recorded as the maker's own counter",
        blocking: true,
        question: "way says whether this is the maker selling or somebody reselling",
        sql: "SELECT e.name || ' at ' || p.name, o.way FROM offerings o\n        JOIN providers p ON p.id = o.provider_id JOIN entities e ON e.id = o.entity_id\n        WHERE o.way = 'api' AND p.kind = 'aggregator' AND COALESCE(e.maker,'') <> p.id",
    },
    Check {
        name: "a name still shaped like a seller's id",
        blocking: true,
        question: "\"anthropic/claude-opus-4-7\" is how a feed writes it, not what it is called",
        sql: "SELECT id, name FROM entities\n        WHERE name LIKE '%/%' OR name LIKE '%: %' OR name LIKE '%@%'",
    },
    Check {
        name: "a live offering nobody has confirmed in a week",
        blocking: false,
        question: "the seller's page was read and this row was not on it; retire should have shelved it, so each one here is the retire job failing to keep up",
        sql: "SELECT e.name || ' at ' || p.name, o.last_seen FROM offerings o\n          JOIN entities e ON e.id=o.entity_id JOIN providers p ON p.id=o.provider_id\n         WHERE o.status='live' AND o.way IN ('api','aggregator','cloud')\n           AND o.last_seen < date((SELECT MAX(taken_at) FROM prices), '-7 day')\n           AND EXISTS (SELECT 1 FROM offerings o2 WHERE o2.provider_id=o.provider_id AND o2.way=o.way\n                        AND o2.last_seen >= date((SELECT MAX(taken_at) FROM prices), '-2 day'))",
    },
    Check {
        name: "a rate far outside its dimension's range",
        blocking: false,
        question: "a hundredfold outlier is usually a per-thousand rate read as a per-million one",
        sql: "WITH latest AS (\n  SELECT x.* FROM prices x JOIN\n    (SELECT offering_id, dimension, MAX(id) mid FROM prices GROUP BY 1,2) l\n    ON x.id = l.mid\n),\n             band AS (SELECT dimension, AVG(micros_per_unit) m, COUNT(*) n\n                      FROM (\n  SELECT x.* FROM prices x JOIN\n    (SELECT offering_id, dimension, MAX(id) mid FROM prices GROUP BY 1,2) l\n    ON x.id = l.mid\n) GROUP BY dimension)\n        SELECT e.name || ' at ' || p.name || ' ' || l.dimension,\n               ROUND(l.micros_per_unit/1e6, 4) || ' vs ' || ROUND(b.m/1e6, 4) || ' average'\n        FROM latest l JOIN band b ON b.dimension = l.dimension\n        JOIN offerings o ON o.id = l.offering_id JOIN entities e ON e.id = o.entity_id\n        JOIN providers p ON p.id = o.provider_id\n        WHERE b.n > 20 AND l.micros_per_unit > 150 * b.m\n        ORDER BY l.micros_per_unit * 1.0 / b.m DESC",
    },
    Check {
        name: "two sources, two prices for the same thing",
        blocking: false,
        question: "the same seller's same rate reported differently by two sources. One of them is wrong and the catalogue cannot tell which, so it shows the seller's own page where there is one and reports the rest",
        sql: "WITH latest AS (\n            SELECT x.offering_id, x.dimension, x.source_url, x.micros_per_unit\n              FROM prices x\n              JOIN (SELECT offering_id, dimension, source_url, MAX(id) mid\n                      FROM prices GROUP BY 1,2,3) k ON k.mid = x.id)\n        SELECT e.name || ' at ' || p.name || ' ' || l.dimension,\n               ROUND(MIN(l.micros_per_unit)/1e6, 4) || ' against ' ||\n               ROUND(MAX(l.micros_per_unit)/1e6, 4) || ', from ' ||\n               COUNT(DISTINCT l.source_url) || ' sources'\n          FROM latest l\n          JOIN offerings o ON o.id = l.offering_id\n          JOIN entities e ON e.id = o.entity_id\n          JOIN providers p ON p.id = o.provider_id\n         GROUP BY l.offering_id, l.dimension\n        HAVING COUNT(DISTINCT l.micros_per_unit) > 1\n         ORDER BY MAX(l.micros_per_unit) * 1.0 / MIN(l.micros_per_unit) DESC",
    },
    Check {
        name: "a plan that grants nothing you can find",
        blocking: true,
        question: "a subscription points at the product it gives you; a pointer to a row that is not there is a plan a reader cannot follow",
        sql: "SELECT id, name FROM entities e\n         WHERE register = 'subscription' AND derived_from IS NOT NULL\n           AND NOT EXISTS (SELECT 1 FROM entities x WHERE x.id = e.derived_from)",
    },
    Check {
        name: "a plan that never says what it allows",
        blocking: true,
        question: "a monthly price with no cap beside it is half a price, and the cap is the reason this register exists",
        sql: "SELECT id, name FROM entities\n         WHERE register = 'subscription'\n           AND COALESCE(json_extract(attrs,'$.limits'), '') = ''",
    },
    Check {
        name: "something given away with no allowance beside it",
        blocking: false,
        question: "a free lane whose ceiling nobody published — true, and not something a reader can act on, so it is counted rather than hidden",
        sql: "SELECT e.name, p.name FROM offerings o\n          JOIN entities e ON e.id = o.entity_id\n          JOIN providers p ON p.id = o.provider_id\n         WHERE o.variant = 'free'\n           AND COALESCE(o.allowance, '{}') IN ('', '{}')",
    },
    Check {
        name: "a free price outside the free lane",
        blocking: true,
        question: "a rate of nought anywhere else is a rounding mistake, and this is the rule that lets the two be told apart",
        sql: "SELECT e.name, x.dimension FROM prices x\n          JOIN offerings o ON o.id = x.offering_id\n          JOIN entities e ON e.id = o.entity_id\n         WHERE x.micros_per_unit = 0 AND COALESCE(o.variant,'') <> 'free'",
    },
    Check {
        name: "a thing no list will ever show",
        blocking: true,
        question: "every entity reaches a reader through its register's hub; one filed under a register no hub covers is a page nothing links to",
        sql: "SELECT id, register FROM entities\n         WHERE register NOT IN ('model','tool','agent','subscription')",
    },
    Check {
        name: "a card that says nothing",
        blocking: true,
        question: "no price, no standing and no description is not an entry, it is a name",
        sql: "SELECT id, name FROM entities e WHERE\n          NOT EXISTS (SELECT 1 FROM docs d WHERE d.subject = e.id AND d.kind = 'description')\n          AND NOT EXISTS (SELECT 1 FROM benchmarks b WHERE b.entity_id = e.id)\n          AND NOT EXISTS (SELECT 1 FROM offerings o JOIN prices x ON x.offering_id = o.id\n                          WHERE o.entity_id = e.id)\n          -- A thing sold only by the month has no price of its own: the price\n          -- is on the plan that grants it, and the card shows those. Devin is\n          -- not an empty entry for having no per-token rate.\n          AND NOT EXISTS (SELECT 1 FROM entities s JOIN offerings o ON o.entity_id = s.id\n                           JOIN prices x ON x.offering_id = o.id\n                          WHERE s.register = 'subscription' AND s.derived_from = e.id)",
    },
    Check {
        name: "an offering with no price",
        blocking: false,
        question: "a way to buy that never says what it costs, other than open weights you run yourself",
        sql: "SELECT e.name || ' at ' || p.name, o.way FROM offerings o\n        JOIN entities e ON e.id = o.entity_id JOIN providers p ON p.id = o.provider_id\n        WHERE o.way <> 'local' AND NOT EXISTS\n          (SELECT 1 FROM prices x WHERE x.offering_id = o.id)",
    },
    Check {
        name: "a standing with no place in a field",
        blocking: false,
        question: "a score means little without knowing how many it beat",
        sql: "WITH latest AS (SELECT b.* FROM benchmarks b JOIN\n            (SELECT entity_id, suite, metric, MAX(id) mid FROM benchmarks GROUP BY 1,2,3) l\n            ON b.id = l.mid)\n        SELECT entity_id || ' on ' || suite, metric FROM latest\n        WHERE rank IS NULL OR out_of IS NULL",
    },
    Check {
        name: "a board that does not say who runs it",
        blocking: true,
        question: "a standing is only as good as the name behind the measurement",
        sql: "SELECT id, name FROM suites WHERE COALESCE(measurer,'') = '' OR COALESCE(url,'') = ''",
    },
    Check {
        name: "a figure nobody has re-read in a while",
        blocking: false,
        question: "a stale price is worse than a missing one, because it reads as current",
        sql: "WITH latest AS (\n  SELECT x.* FROM prices x JOIN\n    (SELECT offering_id, dimension, MAX(id) mid FROM prices GROUP BY 1,2) l\n    ON x.id = l.mid\n)\n        SELECT e.name || ' at ' || p.name, l.taken_at\n        FROM latest l JOIN offerings o ON o.id = l.offering_id\n        JOIN entities e ON e.id = o.entity_id JOIN providers p ON p.id = o.provider_id\n        WHERE l.taken_at < date('now', '-45 day')\n        GROUP BY o.id",
    },
];

/// The twelve that are not one query still have to say what they looked
/// for and whether they block, so their two lines live here.
const CODED_SPEC: &[(&str, bool, &str)] = &[
    ("a fact filed under a name nothing reads", true, "an attribute spelled differently from the one the pages look up is a fact collected into silence"),
    ("held in the pen and in the catalogue at once", true, "the quarantine is a second database so the product cannot reach it; a row in both means a candidate arrived by another road and nobody let it out"),
    ("one maker spelling its own brand two ways", false, "QWEN beside Qwen, DeepSeek beside Deepseek — a reader cannot tell whether two rows are two products"),
    ("one address, two things", true, "a page is addressed /index/<maker>/<product>, so two rows that reduce to the same address are one row twice — this catches what an exact name comparison misses, like \"Qwen3 14B\" beside \"Qwen3-14B\""),
    ("a company that would shadow a reserved address", true, "the first segment of an address is either a company or one of the words the index reserves for its own hubs, and a company named one of those would take the hub's page"),
    ("a name that cannot be addressed", true, "a page is addressed by a slug of the name, and a name in a script the slug cannot carry — 混元生图 — reduces to nothing, which is an address that answers 404"),
    ("two rows one name would reach", false, "a reduced name that two entities both answer to. The resolver refuses to bind it — guessing which of them a price belongs to is worse than missing the price — so each of these costs the catalogue every binding that name would have made"),
    ("a word outside the vocabulary", true, "a register, a way, a provider kind or a modality the rest of the catalogue does not use is a row nothing will ever filter on"),
    ("a row pointing at nothing", true, "an offering, a price, a standing or an alias whose subject was deleted"),
    ("a product with no maker", false, "somebody makes it; a commodity the whole market sells is the exception"),
    ("a tag that contradicts the modality", true, "a thing tagged image that puts out no image, or tagged speak that puts out no audio, was tagged by a rule that over-fired"),
    ("a name with whitespace stuck to it", true, "a name is matched character for character, so a stray tab is a name nothing will ever match — one alias carried one and it read as a disagreement between two resolvers that in fact agreed"),
];

fn rows_of(con: &Connection, sql: &str) -> Result<Vec<Finding>> {
    let mut q = con.prepare(sql)?;
    let n = q.column_count();
    let mut rows = q.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let cell = |i: usize| -> String {
            match r.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Null) => "None".into(),
                Ok(rusqlite::types::ValueRef::Integer(v)) => v.to_string(),
                Ok(rusqlite::types::ValueRef::Real(v)) => v.to_string(),
                Ok(rusqlite::types::ValueRef::Text(t)) => {
                    String::from_utf8_lossy(t).into_owned()
                }
                _ => String::new(),
            }
        };
        out.push((cell(0), if n > 1 { cell(1) } else { String::new() }));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The twelve that are not one query
// ---------------------------------------------------------------------------

/// An address the catalogue can live with. This is check.py's spelling of it,
/// kept character for character so the port is accepted — note that it is NOT
/// the one the server addresses pages with: `slug` in lib.rs keeps a dot, and
/// this turns it into a hyphen.
fn check_slug(s: &str) -> String {
    static R: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (non, runs) = R.get_or_init(|| {
        (
            Regex::new(r"[^a-z0-9]+").unwrap(),
            Regex::new(r"-{2,}").unwrap(),
        )
    });
    let s = s.to_lowercase().replace('+', " plus ").replace('&', " and ");
    let s = non.replace_all(&s, "-");
    runs.replace_all(s.trim_matches('-'), "-").into_owned()
}

/// A key nothing reads is a fact filed where no page will ever look.
fn stray_attr(con: &Connection) -> Result<Vec<Finding>> {
    const KNOWN: &[&str] = &[
        "tasks", "license", "context", "params", "params_read_from",
        "open_weights", "open_weights_source", "limits", "includes",
        "released", "knowledge", "max_output", "reasoning", "tool_call",
    ];
    let mut q = con.prepare("SELECT id, name, COALESCE(attrs,'{}') FROM entities")?;
    let mut rows = q.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let name: String = r.get(1)?;
        let attrs: String = r.get(2)?;
        let Ok(d) = serde_json::from_str::<serde_json::Value>(&attrs) else { continue };
        let Some(map) = d.as_object() else { continue };
        let mut odd: Vec<&String> = map.keys().filter(|k| !KNOWN.contains(&k.as_str())).collect();
        if !odd.is_empty() {
            odd.sort();
            out.push((
                name,
                odd.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            ));
        }
    }
    Ok(out)
}

/// The pen is a second database so the product cannot reach it; a row in both
/// means a candidate arrived by another road and nobody let it out.
fn pen_leak(con: &Connection, db_path: &str) -> Result<Vec<Finding>> {
    let pen = std::path::Path::new(db_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("quarantine.db");
    if !pen.exists() {
        return Ok(vec![]);
    }
    let other = common::db::open(pen.to_str().unwrap_or_default())?;
    let held: Vec<String> = {
        let mut q = other.prepare("SELECT id FROM candidates")?;
        let v: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        v
    };
    if held.is_empty() {
        return Ok(vec![]);
    }
    let marks = vec!["?"; held.len()].join(",");
    let sql = format!(
        "SELECT name, id FROM providers WHERE id IN ({marks}) \
         UNION ALL SELECT name, id FROM entities WHERE id IN ({marks})"
    );
    let both: Vec<&String> = held.iter().chain(held.iter()).collect();
    let mut q = con.prepare(&sql)?;
    let v: Vec<Finding> = q
        .query_map(rusqlite::params_from_iter(both), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

/// One maker writing its own brand two ways. Named here rather than mended,
/// because mending it is `naming`'s job and this only has to notice.
fn brand_spelling(con: &Connection) -> Result<Vec<Finding>> {
    static R: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (lead, slug) = R.get_or_init(|| {
        (
            Regex::new(r"^([A-Za-z]+)").unwrap(),
            Regex::new(r"^[a-z0-9][a-z0-9._\-]*$").unwrap(),
        )
    });
    let mut order: Vec<(String, String)> = Vec::new();
    let mut seen: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut q = con.prepare("SELECT name, COALESCE(maker,'') FROM entities")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let name: String = r.get(0)?;
        let maker: String = r.get(1)?;
        let t = name.trim();
        if slug.is_match(t) {
            continue;
        }
        let Some(c) = lead.captures(t) else { continue };
        let b = c.get(1).unwrap().as_str().to_string();
        let key = (b.to_lowercase(), maker);
        let e = seen.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Vec::new()
        });
        if !e.contains(&b) {
            e.push(b);
        }
    }
    let mut out = Vec::new();
    for key in &order {
        let v = &seen[key];
        if v.len() > 1 {
            let mut sorted = v.clone();
            sorted.sort();
            out.push((
                format!(
                    "{} ({})",
                    key.0,
                    if key.1.is_empty() { "no maker" } else { &key.1 }
                ),
                sorted.join(", "),
            ));
        }
    }
    Ok(out)
}

/// Two things at one address is a page that cannot exist twice.
fn dup_slug(con: &Connection) -> Result<Vec<Finding>> {
    let mut prov: HashMap<String, String> = HashMap::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            let s = check_slug(&name);
            match counts.iter_mut().find(|(x, _)| *x == s) {
                Some((_, n)) => *n += 1,
                None => counts.push((s.clone(), 1)),
            }
            prov.insert(id, s);
        }
    }
    let mut out: Vec<Finding> = counts
        .iter()
        .filter(|(_, n)| *n > 1)
        .map(|(s, _)| (format!("/index/{s}"), "two providers".to_string()))
        .collect();
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut q = con.prepare("SELECT id, name, COALESCE(maker,'') FROM entities")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let eid: String = r.get(0)?;
        let name: String = r.get(1)?;
        let maker: String = r.get(2)?;
        let path = format!(
            "/index/{}/{}",
            prov.get(&maker).map(String::as_str).unwrap_or("commons"),
            check_slug(&name)
        );
        if let Some(first) = seen.get(&path) {
            out.push((path.clone(), format!("{first} + {eid}")));
        }
        seen.insert(path, eid);
    }
    Ok(out)
}

/// A company whose address is a word the catalogue keeps for itself.
fn reserved(con: &Connection) -> Result<Vec<Finding>> {
    const RESERVED: &[&str] = &[
        "board", "task", "licence", "license", "commons", "models", "top",
        "free", "subscriptions", "waiting", "tools", "agents", "providers",
        "search", "about", "sitemap",
    ];
    let mut q = con.prepare("SELECT id, name FROM providers")?;
    let mut rows = q.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let id: String = r.get(0)?;
        let name: String = r.get(1)?;
        let s = check_slug(&name);
        if RESERVED.contains(&s.as_str()) {
            out.push((id, s));
        }
    }
    Ok(out)
}

/// A name with nothing addressable left in it has no page at all.
fn unaddressable(con: &Connection) -> Result<Vec<Finding>> {
    let bare = |s: &str| -> String {
        s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
    };
    let mut out = Vec::new();
    for sql in [
        "SELECT id, name FROM entities",
        "SELECT id, name FROM providers",
    ] {
        let mut q = con.prepare(sql)?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let name: String = r.get(1)?;
            if bare(&name).is_empty() {
                out.push((id, name));
            }
        }
    }
    Ok(out)
}

/// A form two rows both answer to. The resolver refuses to bind it — guessing
/// which of them a price belongs to is worse than missing the price — so each
/// one costs the catalogue every binding it had.
fn ambiguous_names(con: &Connection) -> Result<Vec<Finding>> {
    let r = crate::resolve::Resolver::from_conn(con)?;
    let mut forms: Vec<&String> = r.ambiguous.iter().collect();
    forms.sort();
    let mut out = Vec::new();
    for f in forms {
        let who: Option<String> = con
            .query_row(
                "SELECT GROUP_CONCAT(name, ' | ') FROM ( \
                   SELECT DISTINCT e.name FROM entities e \
                    WHERE e.id IN (SELECT entity_id FROM aliases WHERE alias = ?1) \
                       OR LOWER(REPLACE(REPLACE(REPLACE(e.name,' ',''),'-',''),'.','')) = ?1 \
                    LIMIT 4)",
                [f],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        let text = who.unwrap_or_else(|| "two rows".into());
        out.push((f.clone(), text.chars().take(80).collect()));
    }
    Ok(out)
}

/// A word outside the vocabulary is a column the pages cannot read.
fn vocabulary(con: &Connection) -> Result<Vec<Finding>> {
    let mut bad = Vec::new();
    let distinct = |sql: &str| -> Result<Vec<String>> {
        let mut q = con.prepare(sql)?;
        let v: Vec<String> = q
            .query_map([], |r| r.get::<_, Option<String>>(0).map(|x| x.unwrap_or_default()))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    };
    for k in distinct("SELECT DISTINCT kind FROM providers")? {
        if !PROVIDER_KINDS.contains(&k.as_str()) {
            bad.push(("provider kind".to_string(), k));
        }
    }
    for w in distinct("SELECT DISTINCT way FROM offerings")? {
        if !WAYS.contains(&w.as_str()) {
            bad.push(("way".to_string(), w));
        }
    }
    for reg in distinct("SELECT DISTINCT register FROM entities")? {
        if !REGISTERS.contains(&reg.as_str()) {
            bad.push(("register".to_string(), reg));
        }
    }
    // A subscription takes nothing and returns nothing: it is a plan, not a
    // thing that transforms data, and the modality columns are empty on
    // purpose. Everything else must name a modality the catalogue knows.
    for col in ["input_kind", "output_kind"] {
        let sql = format!(
            "SELECT DISTINCT {col} FROM entities WHERE register <> 'subscription'"
        );
        for v in distinct(&sql)? {
            if v.split('+').any(|p| !KINDS.contains(&p.trim())) {
                bad.push((col.to_string(), v));
            }
        }
    }
    Ok(bad)
}

/// A row pointing at nothing: the join that would show it finds no other end.
fn orphans(con: &Connection) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    for (what, sql) in [
        ("offering with no entity",
         "SELECT o.id, o.entity_id FROM offerings o WHERE NOT EXISTS \
          (SELECT 1 FROM entities e WHERE e.id = o.entity_id)"),
        ("price with no offering",
         "SELECT x.id, x.dimension FROM prices x WHERE NOT EXISTS \
          (SELECT 1 FROM offerings o WHERE o.id = x.offering_id)"),
        ("standing with no entity",
         "SELECT b.id, b.entity_id FROM benchmarks b WHERE NOT EXISTS \
          (SELECT 1 FROM entities e WHERE e.id = b.entity_id)"),
        ("alias with no entity",
         "SELECT a.alias, a.entity_id FROM aliases a WHERE NOT EXISTS \
          (SELECT 1 FROM entities e WHERE e.id = a.entity_id)"),
        ("offering with no provider",
         "SELECT o.id, o.provider_id FROM offerings o WHERE NOT EXISTS \
          (SELECT 1 FROM providers p WHERE p.id = o.provider_id)"),
    ] {
        for (a, b) in rows_of(con, sql)? {
            out.push((what.to_string(), format!("{a} {b}")));
        }
    }
    Ok(out)
}

/// A product with no maker, excepting the commons.
fn no_maker(con: &Connection) -> Result<Vec<Finding>> {
    let list = COMMONS
        .iter()
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(",");
    rows_of(
        con,
        &format!("SELECT id, name FROM entities WHERE COALESCE(maker,'') = '' AND id NOT IN ({list})"),
    )
}

/// A thing tagged image that puts out no image, or tagged speak that puts out
/// no audio, was tagged by a rule that over-fired.
fn tag_conflict(con: &Connection) -> Result<Vec<Finding>> {
    const PAIRS: &[(&str, &str)] = &[
        ("image", "image"), ("video", "video"), ("avatar", "video"),
        ("speak", "audio"), ("music", "audio"), ("embedding", "embedding"),
    ];
    let mut q = con.prepare(
        "SELECT id, name, output_kind, json_extract(attrs,'$.tasks') FROM entities \
          WHERE json_extract(attrs,'$.tasks') IS NOT NULL",
    )?;
    let mut rows = q.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let name: String = r.get(1)?;
        let outk: Option<String> = r.get(2)?;
        let tasks: Option<String> = r.get(3)?;
        let tasks = tasks.unwrap_or_default();
        let outk = outk.unwrap_or_default();
        for (tag, needs) in PAIRS {
            if tasks.contains(&format!("\"{tag}\"")) && !outk.contains(needs) {
                out.push((name.clone(), format!("tagged {tag} but puts out {outk}")));
            }
        }
    }
    Ok(out)
}

/// A name is matched character for character, so a stray tab is a name
/// nothing will ever match.
fn ragged_name(con: &Connection) -> Result<Vec<Finding>> {
    let edge = "char(9)||char(10)||char(13)||' '";
    let mut out = Vec::new();
    for (table, col) in [("entities", "name"), ("providers", "name"), ("aliases", "alias")] {
        let sql = format!("SELECT {col} FROM {table} WHERE {col} <> trim({col}, {edge})");
        for (v, _) in rows_of(con, &sql)? {
            out.push((format!("{table}.{col}"), format!("{v:?}")));
        }
    }
    Ok(out)
}

/// The checks in the order they are declared, because the report reads in
/// that order and the two have to print the same thing.
const ORDER: &[(&str, Option<&str>)] = &[
    ("one model string, two entities", None),
    ("one name, two entities under one maker", None),
    ("a row that contradicts itself about the weights", None),
    ("a fact filed under a name nothing reads", Some("stray_attr")),
    ("a thing whose maker is gone", None),
    ("two descriptions for one thing", None),
    ("a description that says nothing", None),
    ("held in the pen and in the catalogue at once", Some("pen_leak")),
    ("published before its own training data", None),
    ("a seller credited as the maker", None),
    ("one lane written twice at one price", None),
    ("one maker spelling its own brand two ways", Some("brand_spelling")),
    ("one name, two providers", None),
    ("one standing under two labels", None),
    ("one address, two things", Some("dup_slug")),
    ("a company that would shadow a reserved address", Some("reserved")),
    ("a name that cannot be addressed", Some("unaddressable")),
    ("a price of nought", None),
    ("a resale recorded as the maker's own counter", None),
    ("a name still shaped like a seller's id", None),
    ("a rate far outside its dimension's range", None),
    ("two sources, two prices for the same thing", None),
    ("two rows one name would reach", Some("ambiguous_names")),
    ("a word outside the vocabulary", Some("vocabulary")),
    ("a plan that grants nothing you can find", None),
    ("a plan that never says what it allows", None),
    ("something given away with no allowance beside it", None),
    ("a free price outside the free lane", None),
    ("a thing no list will ever show", None),
    ("a row pointing at nothing", Some("orphans")),
    ("a card that says nothing", None),
    ("an offering with no price", None),
    ("a product with no maker", Some("no_maker")),
    ("a standing with no place in a field", None),
    ("a board that does not say who runs it", None),
    ("a figure nobody has re-read in a while", None),
    ("a tag that contradicts the modality", Some("tag_conflict")),
    ("a name with whitespace stuck to it", Some("ragged_name")),];

/// What one check found, kept so the coverage page can show the same verdict
/// that would have stopped the nightly run rather than a claim about it.
pub struct Verdict {
    pub name: &'static str,
    pub blocking: bool,
    pub findings: i64,
    pub asks: &'static str,
}

fn run_one(con: &Connection, name: &str, coded: Option<&str>, db: &str) -> Result<Vec<Finding>> {
    match coded {
        Some("stray_attr") => stray_attr(con),
        Some("pen_leak") => pen_leak(con, db),
        Some("brand_spelling") => brand_spelling(con),
        Some("dup_slug") => dup_slug(con),
        Some("reserved") => reserved(con),
        Some("unaddressable") => unaddressable(con),
        Some("ambiguous_names") => ambiguous_names(con),
        Some("vocabulary") => vocabulary(con),
        Some("orphans") => orphans(con),
        Some("no_maker") => no_maker(con),
        Some("tag_conflict") => tag_conflict(con),
        Some("ragged_name") => ragged_name(con),
        Some(other) => anyhow::bail!("no check called {other}"),
        None => {
            let c = QUERIES
                .iter()
                .find(|c| c.name == name)
                .ok_or_else(|| anyhow::anyhow!("no query for {name}"))?;
            rows_of(con, c.sql)
        }
    }
}

fn spec(name: &str) -> (bool, &'static str) {
    QUERIES
        .iter()
        .find(|c| c.name == name)
        .map(|c| (c.blocking, c.question))
        .unwrap_or_else(|| CODED_SPEC.iter().find(|(n, _, _)| *n == name)
            .map(|(_, b, q)| (*b, *q))
            .unwrap_or((true, "")))
}

/// Run every check, print the report, and say how many blocked.
pub fn run(con: &Connection, db: &str) -> Result<(usize, usize, Vec<Verdict>)> {
    println!("Pass Index consistency — {db}");
    let count = |t: &str| -> i64 {
        con.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
            .unwrap_or(0)
    };
    println!(
        "  {} entities, {} providers, {} ways, {} prices, {} standings",
        count("entities"), count("providers"), count("offerings"),
        count("prices"), count("benchmarks")
    );
    println!();

    let (mut blocked, mut warned) = (0usize, 0usize);
    let mut verdicts = Vec::new();
    for (name, coded) in ORDER {
        let (blocking, question) = spec(name);
        let rows = match run_one(con, name, *coded, db) {
            Ok(r) => r,
            Err(e) => {
                // A broken check is a finding too.
                println!("  BROKEN  {name}: {e}");
                blocked += 1;
                verdicts.push(Verdict { name, blocking, findings: -1, asks: question });
                continue;
            }
        };
        verdicts.push(Verdict { name, blocking, findings: rows.len() as i64, asks: question });
        if rows.is_empty() {
            println!("  ok      {name}");
            continue;
        }
        if blocking { blocked += 1 } else { warned += 1 }
        println!("  {}    {name}: {}", if blocking { "FAIL" } else { "warn" }, rows.len());
        println!("            {question}");
        for (a, b) in rows.iter().take(5) {
            println!("            · {a} — {b}");
        }
        if rows.len() > 5 {
            println!("            … and {} more", rows.len() - 5);
        }
    }
    println!();
    println!("{blocked} blocking, {warned} worth knowing");
    Ok((blocked, warned, verdicts))
}

/// Write this run's verdict where the coverage page can read it. A page that
/// says everything is in place, and could not say otherwise, is decoration.
pub fn record(con: &Connection, suite: &str, v: &[Verdict]) -> Result<()> {
    con.execute_batch(
        "CREATE TABLE IF NOT EXISTS checks (
            name TEXT PRIMARY KEY, suite TEXT NOT NULL, blocking INTEGER NOT NULL DEFAULT 0,
            findings INTEGER NOT NULL DEFAULT 0, asks TEXT NOT NULL DEFAULT '',
            ran_at TEXT NOT NULL)",
    )?;
    con.execute("DELETE FROM checks WHERE suite=?1", [suite])?;
    for x in v {
        con.execute(
            "INSERT OR REPLACE INTO checks (name,suite,blocking,findings,asks,ran_at) \
             VALUES (?1,?2,?3,?4,?5,date('now'))",
            rusqlite::params![x.name, suite, x.blocking as i64, x.findings, x.asks],
        )?;
    }
    Ok(())
}

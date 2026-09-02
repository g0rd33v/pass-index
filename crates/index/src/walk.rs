//! What a reader is actually handed.
//!
//! The database can be consistent and the pages still broken: an address that
//! 404s, a page that stopped saying when it was read, two pages competing for
//! one title, a link that leads nowhere. `checks` reads the catalogue; this
//! reads the site, over HTTP, the way a crawler does.
//!
//! Every rule here is one that has been broken at least once. A list of only
//! the failures cannot say that anything is in place, so every rule is
//! reported whether it fired or not.

use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

/// A page that weighs more than this compressed is a page somebody on a train
/// gives up on. Compressed is the point: it is what the reader's connection
/// actually carries.
const MAX_BYTES: usize = 60_000;
/// What a search result will show before it cuts.
const TITLE_MAX: usize = 70;
const DESC_MIN: usize = 60;
const DESC_MAX: usize = 320;

pub struct Rule {
    pub name: &'static str,
    pub blocking: bool,
    pub asks: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule { name: "an address in the sitemap that does not answer", blocking: true,
           asks: "every address the sitemap offers returns a page" },
    Rule { name: "a page with no title", blocking: true, asks: "every page names itself" },
    Rule { name: "a page with no description", blocking: true,
           asks: "every page carries a search snippet" },
    Rule { name: "a page with no canonical", blocking: true,
           asks: "every page declares its own address" },
    Rule { name: "a canonical that points elsewhere", blocking: true,
           asks: "no page claims to be another" },
    Rule { name: "a page without exactly one h1", blocking: true,
           asks: "one heading, once, per page" },
    Rule { name: "a page that never says when it was read", blocking: true,
           asks: "every page dates the figures it prints" },
    Rule { name: "markup printed as text", blocking: true,
           asks: "nothing escaped twice into the body" },
    Rule { name: "structured data that does not parse", blocking: true,
           asks: "the machine-readable block is valid" },
    Rule { name: "a JSON twin that does not answer", blocking: true,
           asks: "every page has the twin its footer promises" },
    Rule { name: "a JSON twin that does not parse", blocking: true,
           asks: "that twin is valid JSON" },
    Rule { name: "one title on several pages", blocking: true,
           asks: "no two pages compete for one reader" },
    Rule { name: "a link on a page that leads nowhere", blocking: true,
           asks: "every link the pages offer resolves" },
    Rule { name: "a page too heavy for a phone", blocking: false,
           asks: "no page is heavier than a train ride allows" },
    Rule { name: "a title too long to survive a search result", blocking: false,
           asks: "titles survive a search result" },
    Rule { name: "a description of the wrong length to quote", blocking: false,
           asks: "descriptions are quotable" },
    Rule { name: "one description on several pages", blocking: false,
           asks: "descriptions are not shared" },
    Rule { name: "a heading over nothing", blocking: false,
           asks: "no heading stands over an empty list" },
    Rule { name: "a page linked but not in the sitemap", blocking: false,
           asks: "everything linked is also listed" },
];

struct Pats {
    title: Regex,
    h1: Regex,
    desc: Regex,
    canonical: Regex,
    date: Regex,
    escaped: Regex,
    empty_head: Regex,
    ld: Regex,
    href: Regex,
    loc: Regex,
    tag: Regex,
}

fn pats() -> &'static Pats {
    static P: OnceLock<Pats> = OnceLock::new();
    P.get_or_init(|| Pats {
        title: Regex::new(r"(?s)<title[^>]*>(.*?)</title>").unwrap(),
        h1: Regex::new(r"<h1[ >]").unwrap(),
        desc: Regex::new(r#"<meta name="description" content="([^"]*)""#).unwrap(),
        canonical: Regex::new(r#"<link rel="canonical" href="([^"]*)""#).unwrap(),
        date: Regex::new(r"20\d\d-\d\d-\d\d").unwrap(),
        escaped: Regex::new(r"&lt;(span|div|a|b|p)\b").unwrap(),
        empty_head: Regex::new(r"(?s)<(h2)[^>]*>[^<]*</h2>\s*<(?:ul|table|div)[^>]*>\s*</")
            .unwrap(),
        ld: Regex::new(r#"(?s)<script type="application/ld\+json">(.*?)</script>"#).unwrap(),
        // Two hashes: the class itself contains `"#`, which would close a
        // single-hash raw string in the middle of the pattern.
        href: Regex::new(r##"href="(/index[^"#?]*)""##).unwrap(),
        loc: Regex::new(r"<loc>[^<]*?(/index[^<]*)</loc>").unwrap(),
        tag: Regex::new(r"<[^>]+>").unwrap(),
    })
}

/// What arrived, and what it weighed on the wire.
struct Got {
    status: u16,
    wire: usize,
    body: String,
}

/// The rule asks what a reader receives, and every reader comes through nginx,
/// which compresses. Reading the container directly and counting uncompressed
/// bytes measured a number nobody receives, so the body is compressed here and
/// the check means the same thing wherever it is pointed.
fn wire_bytes(raw: &[u8]) -> usize {
    use std::io::Write;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    if enc.write_all(raw).is_err() {
        return raw.len();
    }
    enc.finish().map(|v| v.len()).unwrap_or(raw.len())
}

async fn fetch(client: &reqwest::Client, base: &str, path: &str) -> Got {
    match client.get(format!("{base}{path}")).send().await {
        Ok(r) => {
            let status = r.status().as_u16();
            let raw = r.bytes().await.unwrap_or_default();
            Got {
                status,
                wire: wire_bytes(&raw),
                body: String::from_utf8_lossy(&raw).into_owned(),
            }
        }
        // A source that could not be reached produced nothing, and that is
        // the fact worth keeping: silence is the failure mode that hides.
        Err(e) => Got { status: 0, wire: 0, body: e.to_string() },
    }
}

fn text_of(re: &Regex, html: &str) -> Option<String> {
    re.captures(html).map(|c| {
        pats()
            .tag
            .replace_all(c.get(1).unwrap().as_str(), "")
            .trim()
            .to_string()
    })
}

#[derive(Default)]
pub struct Findings {
    rows: Vec<(&'static str, String, String)>,
}

impl Findings {
    fn add(&mut self, rule: &'static str, page: &str, detail: String) {
        self.rows.push((rule, page.to_string(), detail));
    }

    /// Every rule and how many times it was broken, pass or fail.
    pub fn verdicts(&self) -> Vec<(&'static str, bool, i64, &'static str)> {
        RULES
            .iter()
            .map(|r| {
                let n = self.rows.iter().filter(|(name, _, _)| *name == r.name).count();
                (r.name, r.blocking, n as i64, r.asks)
            })
            .collect()
    }

    /// Blocking first, then by name, so the report reads the same every time.
    pub fn report(&self) -> usize {
        let mut names: Vec<&'static str> = Vec::new();
        for (n, _, _) in &self.rows {
            if !names.contains(n) {
                names.push(n);
            }
        }
        let blocking = |n: &str| RULES.iter().find(|r| r.name == n).is_some_and(|r| r.blocking);
        names.sort_by_key(|n| (!blocking(n), *n));
        let mut blocked = 0;
        for name in names {
            let items: Vec<_> = self.rows.iter().filter(|(n, _, _)| *n == name).collect();
            if blocking(name) {
                blocked += 1;
            }
            println!(
                "  {}    {name}: {}",
                if blocking(name) { "FAIL" } else { "warn" },
                items.len()
            );
            for (_, page, detail) in items.iter().take(5) {
                println!("            · {page} — {detail}");
            }
            if items.len() > 5 {
                println!("            … and {} more", items.len() - 5);
            }
        }
        blocked
    }
}

/// One page, read the way a crawler reads it.
struct Seen {
    findings: Vec<(&'static str, String, String)>,
    title: Option<String>,
    desc: Option<String>,
    links: Vec<String>,
}

async fn check_page(client: &reqwest::Client, base: &str, path: &str) -> Seen {
    let p = pats();
    let mut out = Seen { findings: Vec::new(), title: None, desc: None, links: Vec::new() };
    let mut note = |rule: &'static str, detail: String| {
        out.findings.push((rule, path.to_string(), detail));
    };

    let got = fetch(client, base, path).await;
    if got.status != 200 {
        note("an address in the sitemap that does not answer", format!("HTTP {}", got.status));
        return out;
    }
    if got.wire > MAX_BYTES {
        note("a page too heavy for a phone", format!("{} bytes on the wire", got.wire));
    }
    if path.ends_with(".xml") {
        return out;
    }
    let html = &got.body;

    match text_of(&p.title, html) {
        None => note("a page with no title", "none".into()),
        Some(t) if t.is_empty() => note("a page with no title", "none".into()),
        Some(t) => {
            if t.chars().count() > TITLE_MAX {
                note(
                    "a title too long to survive a search result",
                    format!("{} chars", t.chars().count()),
                );
            }
            out.title = Some(t);
        }
    }

    match p.desc.captures(html).map(|c| c.get(1).unwrap().as_str().to_string()) {
        None => note("a page with no description", "none".into()),
        Some(d) if d.is_empty() => note("a page with no description", "none".into()),
        Some(d) => {
            let n = d.chars().count();
            if !(DESC_MIN..=DESC_MAX).contains(&n) {
                note("a description of the wrong length to quote", format!("{n} chars"));
            }
            out.desc = Some(d);
        }
    }

    match p.canonical.captures(html).map(|c| c.get(1).unwrap().as_str().to_string()) {
        None => note("a page with no canonical", "none".into()),
        Some(c) if !c.ends_with(path) => note("a canonical that points elsewhere", c),
        Some(_) => {}
    }

    let heads = p.h1.find_iter(html).count();
    if heads != 1 {
        note("a page without exactly one h1", format!("{heads} of them"));
    }

    // A catalogue of prices that will not say when it read them is a rumour.
    // The date lives on the status page, which every page links to, rather
    // than in the foot of all four thousand of them.
    if path == "/index/coverage" && !p.date.is_match(html) {
        note("a page that never says when it was read", "no date anywhere".into());
    }

    // Markup that reached the reader as text, which is what double-escaping
    // looks like from the outside.
    if p.escaped.is_match(html) {
        note("markup printed as text", "escaped tag in the body".into());
    }
    for m in p.empty_head.captures_iter(html) {
        note("a heading over nothing", m.get(1).unwrap().as_str().to_string());
    }
    for m in p.ld.captures_iter(html) {
        if let Err(e) = serde_json::from_str::<serde_json::Value>(m.get(1).unwrap().as_str()) {
            note(
                "structured data that does not parse",
                e.to_string().chars().take(60).collect(),
            );
        }
    }

    // The JSON twin every page advertises must exist and parse. Twins are
    // open to anyone — only the whole catalogue in one file asks for an
    // account — so 200 is the expected answer. A refusal that names the
    // sign-in door is tolerated, so gating a file later does not break the
    // walk.
    if path != "/index" && !path.ends_with(".json") {
        let twin = format!("{path}.json");
        let got = fetch(client, base, &twin).await;
        if got.status == 200 {
            if serde_json::from_str::<serde_json::Value>(&got.body).is_err() {
                out.findings.push((
                    "a JSON twin that does not parse",
                    twin,
                    "invalid".into(),
                ));
            }
        } else if !((got.status == 401 || got.status == 303) && got.body.contains("signin")) {
            out.findings.push((
                "a JSON twin that does not answer",
                twin,
                format!("HTTP {}", got.status),
            ));
        }
    }

    // A JSON twin is for machines that already know the page; it belongs in
    // no sitemap, so finding one unlisted is not a finding.
    for m in p.href.captures_iter(html) {
        let href = m.get(1).unwrap().as_str();
        if !href.ends_with(".json") && !out.links.iter().any(|l| l == href) {
            out.links.push(href.to_string());
        }
    }
    out
}

/// Walk every address the sitemap offers and report what a reader would find.
pub async fn walk(base: &str, workers: usize) -> Result<(usize, usize, Findings)> {
    let client = reqwest::Client::builder()
        .user_agent("pass-index-audit/1.0")
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

    let sm = fetch(&client, base, "/index/sitemap.xml").await;
    if sm.status != 200 {
        anyhow::bail!("no sitemap; nothing to walk");
    }
    let addresses: Vec<String> = pats()
        .loc
        .captures_iter(&sm.body)
        .map(|c| c.get(1).unwrap().as_str().to_string())
        .collect();
    println!("Pass Index pages — {base}");
    println!("  {} addresses in the sitemap", addresses.len());
    println!();

    let mut f = Findings::default();
    let mut titles: HashMap<String, Vec<String>> = HashMap::new();
    let mut descs: HashMap<String, Vec<String>> = HashMap::new();
    let mut linked: Vec<String> = Vec::new();

    // Addresses in sitemap order, `workers` at a time, so the report reads
    // the same on every run rather than in whatever order the server replied.
    for chunk in addresses.chunks(workers.max(1)) {
        let seen = futures_util::future::join_all(
            chunk.iter().map(|p| check_page(&client, base, p)),
        )
        .await;
        for (path, s) in chunk.iter().zip(seen) {
            f.rows.extend(s.findings);
            if let Some(t) = s.title {
                titles.entry(t).or_default().push(path.clone());
            }
            if let Some(d) = s.desc {
                descs.entry(d).or_default().push(path.clone());
            }
            for l in s.links {
                if !linked.contains(&l) {
                    linked.push(l);
                }
            }
        }
    }

    // Two pages saying the same thing compete for the same reader.
    let mut shared: Vec<(&String, &Vec<String>)> =
        titles.iter().filter(|(_, p)| p.len() > 1).collect();
    shared.sort_by_key(|(_, p)| p[0].clone());
    for (_, pages) in shared {
        f.add("one title on several pages", &pages[0], format!("also {}", pages[1]));
    }
    let mut shared: Vec<(&String, &Vec<String>)> =
        descs.iter().filter(|(_, p)| p.len() > 1).collect();
    shared.sort_by_key(|(_, p)| p[0].clone());
    for (_, pages) in shared {
        f.add("one description on several pages", &pages[0], format!("also {}", pages[1]));
    }

    // Links the pages offer that the sitemap never mentions: either the link
    // is broken or the sitemap is incomplete, and both are worth knowing.
    let mut stray: Vec<String> =
        linked.into_iter().filter(|l| !addresses.contains(l)).collect();
    stray.sort();
    for path in stray.iter().take(40) {
        let got = fetch(&client, base, path).await;
        if got.status != 200 {
            f.add("a link on a page that leads nowhere", path, format!("HTTP {}", got.status));
        } else {
            f.add("a page linked but not in the sitemap", path, "reachable, unlisted".into());
        }
    }

    let blocked = f.report();
    println!();
    println!("{} pages walked, {blocked} rules broken in a way that blocks", addresses.len());
    Ok((addresses.len(), blocked, f))
}

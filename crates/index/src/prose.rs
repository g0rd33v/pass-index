//! What is written about a company rather than priced by one.
//!
//! Funding rounds, portfolios, who backed whom, and the sentence a company
//! writes about itself. These read prose rather than a rate card, and a prose
//! parser that drifts is silent — it keeps returning something, just not the
//! right thing — so every rule here is narrow and says what it refuses.

use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

/// Words a company adds to its own name and drops in conversation.
fn suffix() -> &'static Regex {
    static S: OnceLock<Regex> = OnceLock::new();
    S.get_or_init(|| {
        Regex::new(
            r"(?i)^(ai|inc|incorporated|corp|corporation|ltd|limited|llc|plc|group|labs?|technologies|technology|systems|company|co|holdings|sa|gmbh|bv|oy|ab)$",
        )
        .unwrap()
    })
}

pub fn words_of(name: &str) -> Vec<String> {
    static W: OnceLock<Regex> = OnceLock::new();
    let re = W.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap());
    re.split(&name.to_lowercase())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether two names are one company.
///
/// Compared as words, not as letters. A company adds words to its own name
/// and drops them in conversation — Perplexity is Perplexity AI — but a word
/// that was never separate is part of the name: one portfolio lists a company
/// called "Open", and letter-wise "open" + "ai" is "openai", which handed
/// OpenAI a batch it never had and three investors it never took.
pub fn same_company(a: &str, b: &str) -> bool {
    let (mut x, mut y) = (words_of(a), words_of(b));
    if x == y {
        return true;
    }
    if x.len() > y.len() {
        std::mem::swap(&mut x, &mut y);
    }
    if x.is_empty() || y[..x.len()] != x[..] {
        return false;
    }
    y[x.len()..].iter().all(|w| suffix().is_match(w))
}

// ---------------------------------------------------------------------------
// One fund's portfolio, read off its own job board
// ---------------------------------------------------------------------------

pub const DVC_FUND: (&str, &str, &str) = (
    "fund_davidovs-venture-collective",
    "Davidovs Venture Collective",
    "https://dvc.ai",
);
pub const DVC_SOURCE: &str = "https://platform.davidovs.com/jobs";
pub const DVC_PORTFOLIO_JSON: &str = include_str!("../data/dvc_portfolio.json");

/// A row of the filter that is not a company.
const NOT_A_COMPANY: &[&str] = &["the team", "animation", "essence"];

struct DvcPats {
    acquired: Regex,
    formerly: Regex,
    aside: Regex,
    non: Regex,
}

fn dvc_pats() -> &'static DvcPats {
    static P: OnceLock<DvcPats> = OnceLock::new();
    P.get_or_init(|| DvcPats {
        acquired: Regex::new(r"(?i)\(\s*acquired by ([^)]+)\)").unwrap(),
        formerly: Regex::new(r"(?i)[(,]\s*ex[- ]?\)?\s*([^),]+)\)?").unwrap(),
        aside: Regex::new(r"\s*\([^)]*\)").unwrap(),
        non: Regex::new(r"[^a-z0-9]+").unwrap(),
    })
}

/// The company's name now, what it used to be called, and who bought it.
pub fn read_portfolio_row(raw: &str) -> (String, Option<String>, Option<String>) {
    let p = dvc_pats();
    let acq = p
        .acquired
        .captures(raw)
        .map(|c| c.get(1).unwrap().as_str().trim().to_string());
    let was = p.formerly.captures(raw).map(|c| {
        c.get(1)
            .unwrap()
            .as_str()
            .trim_matches([' ', ',', '-'])
            .to_string()
    });
    let name = p.acquired.replace_all(raw, "");
    let name = p.formerly.replace_all(&name, "");
    let name = p.aside.replace_all(&name, "");
    let name = name.trim_matches([' ', ',', '-', '—']).to_string();
    (name, was, acq)
}

pub fn provider_id(name: &str) -> String {
    format!(
        "prov_{}",
        dvc_pats()
            .non
            .replace_all(&name.to_lowercase(), "-")
            .trim_matches('-')
    )
}

pub struct PortfolioRow {
    pub held: Option<String>,
    pub name: String,
    pub was: Option<String>,
    pub acquired_by: Option<String>,
}

pub fn dvc_portfolio(con: &rusqlite::Connection) -> Result<(Vec<PortfolioRow>, Vec<String>)> {
    let names: Vec<String> = serde_json::from_str(DVC_PORTFOLIO_JSON)?;
    let mut held: Vec<(String, String)> = Vec::new();
    {
        let mut q = con.prepare("SELECT id, name FROM providers")?;
        let mut rows = q.query([])?;
        while let Some(r) = rows.next()? {
            held.push((r.get(0)?, r.get(1)?));
        }
    }
    let (mut out, mut skipped) = (Vec::new(), Vec::new());
    for raw in &names {
        let (name, was, acq) = read_portfolio_row(raw);
        if name.is_empty() || NOT_A_COMPANY.contains(&name.to_lowercase().as_str()) {
            skipped.push(raw.clone());
            continue;
        }
        let hit = held
            .iter()
            .find(|(_, hn)| same_company(&name, hn))
            .map(|(pid, _)| pid.clone());
        out.push(PortfolioRow { held: hit, name, was, acquired_by: acq });
    }
    Ok((out, skipped))
}

/// A company a fund names in its own portfolio has been looked at by somebody
/// who put money in it, which is why the edge is worth recording even where
/// nothing about the company is priced.
pub fn write_portfolio(con: &rusqlite::Connection, rows: &[PortfolioRow]) -> Result<usize> {
    let (fid, fname, furl) = DVC_FUND;
    con.execute(
        "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,?3,'fund')",
        (fid, fname, furl),
    )?;
    con.execute(
        "UPDATE providers SET url=?1, kind='fund' WHERE id=?2",
        (furl, fid),
    )?;
    let mut added = 0usize;
    for r in rows {
        let pid = r.held.clone().unwrap_or_else(|| provider_id(&r.name));
        if r.held.is_none() {
            con.execute(
                "INSERT OR IGNORE INTO providers (id,name,url,kind,backing,status,listed) \
                 VALUES (?1,?2,'','vendor',?3,?4,0)",
                rusqlite::params![
                    &pid,
                    &r.name,
                    fname,
                    if r.acquired_by.is_some() { "Acquired" } else { "Active" }
                ],
            )?;
            added += 1;
        } else if r.acquired_by.is_some() {
            con.execute("UPDATE providers SET status='Acquired' WHERE id=?1", [&pid])?;
        }
        con.execute(
            "UPDATE providers SET backing=COALESCE(backing,?1) WHERE id=?2",
            (fname, &pid),
        )?;
        if let Some(was) = &r.was {
            con.execute(
                "INSERT OR IGNORE INTO docs (subject,kind,field,text,source_url,taken_at) \
                 VALUES (?1,'fact','formerly',?2,?3,date('now'))",
                rusqlite::params![&pid, was, DVC_SOURCE],
            )?;
        }
        if let Some(acq) = &r.acquired_by {
            con.execute(
                "INSERT OR IGNORE INTO docs (subject,kind,field,text,source_url,taken_at) \
                 VALUES (?1,'fact','acquired_by',?2,?3,date('now'))",
                rusqlite::params![&pid, acq, DVC_SOURCE],
            )?;
        }
        con.execute(
            "INSERT OR IGNORE INTO investments (fund_id,company_id,source_url) \
             VALUES (?1,?2,?3)",
            rusqlite::params![fid, &pid, DVC_SOURCE],
        )?;
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mistake this rule exists to prevent: a company called "Open" is
    /// not OpenAI, however the letters run together.
    #[test]
    fn a_name_is_words_not_letters() {
        assert!(same_company("Perplexity", "Perplexity AI"));
        assert!(same_company("Anthropic", "Anthropic"));
        assert!(!same_company("Open", "OpenAI"));
        assert!(!same_company("Pine AI", "Pine Labs"));
    }

    #[test]
    fn a_portfolio_row_carries_three_facts() {
        assert_eq!(
            read_portfolio_row("API Nexus (Acquired by Perplexity)"),
            ("API Nexus".into(), None, Some("Perplexity".into()))
        );
        assert_eq!(
            read_portfolio_row("Aviary AI (ex-Cambio)"),
            ("Aviary AI".into(), Some("Cambio".into()), None)
        );
        assert_eq!(read_portfolio_row("Etched"), ("Etched".into(), None, None));
    }
}

// ---------------------------------------------------------------------------
// Which companies run on venture money
// ---------------------------------------------------------------------------

pub const WIKI: &str = "https://en.wikipedia.org/w/api.php";
pub const PROSE_UA: &str = "pass-index/1.0 (catalogue of AI prices; https://pass.io)";

struct RoundPats {
    raise: Regex,
    round: Regex,
    not_a_round: Regex,
    money: Regex,
    sentence: Regex,
    aside: Regex,
    space: Regex,
    is_org: Regex,
}

fn rp() -> &'static RoundPats {
    static P: OnceLock<RoundPats> = OnceLock::new();
    P.get_or_init(|| RoundPats {
        // "raised $124 million", "closed a $500 million round", "secured £8m"
        raise: Regex::new(
            r"(?i)\b(rais\w+|secur\w+|clos\w+|announc\w+|receiv\w+|obtain\w+|led a|completed)\b",
        ).unwrap(),
        round: Regex::new(
            r"(?i)\b(round|funding|seed|pre-seed|series\s+[a-h]\b|investment|financing)\b",
        ).unwrap(),
        // A valuation is not money received, and neither is revenue.
        not_a_round: Regex::new(
            r"(?i)\b(valu\w+|worth|revenue|ARR|market cap|fine|penalt\w+|contract|deal worth|acquisition|acquired for|purchase price|lawsuit|damages)\b",
        ).unwrap(),
        // The amount is either a thousands-grouped run ("110,000,000") or a
        // plain decimal ("1.5"); the grouped form is tried first so the whole
        // number is captured, not its first group. The earlier
        // `[\d]+(?:[.,]\d+)?` read $110,000,000 as 110,000 — a thousand-fold
        // understatement that passed the floor and reached the card.
        money: Regex::new(
            r"(?i)(?:US\$|\$|€|£)\s?([\d]{1,3}(?:,\d{3})+|[\d]+(?:\.\d+)?)\s*(billion|million|bn\b|m\b|trillion)?",
        ).unwrap(),
        // Rust has no look-behind, so a sentence break is a full stop plus
        // whitespace and the stop is kept with the sentence before it.
        sentence: Regex::new(r"\.\s+").unwrap(),
        aside: Regex::new(r"\s*\([^)]*\)\s*$").unwrap(),
        space: Regex::new(r"\s+").unwrap(),
        is_org: Regex::new(r"(?i)\b(company|startup|start-up|corporation|laborator)").unwrap(),
    })
}

fn scale_of(unit: Option<&str>) -> f64 {
    match unit.map(str::to_lowercase).as_deref() {
        Some("trillion") => 1e12,
        Some("billion") | Some("bn") => 1e9,
        Some("million") | Some("m") => 1e6,
        _ => 1.0,
    }
}

/// Split into sentences the way Python's lookbehind does: after a full stop
/// followed by whitespace, with the stop left on the sentence it ends.
pub fn sentences(text: &str) -> Vec<String> {
    let re = &rp().sentence;
    let mut out = Vec::new();
    let mut last = 0usize;
    for m in re.find_iter(text) {
        out.push(text[last..m.start() + 1].to_string());
        last = m.end();
    }
    out.push(text[last..].to_string());
    out
}

/// Every sentence that states a round, and what it stated.
pub fn rounds_in(text: &str) -> Vec<(f64, String)> {
    let p = rp();
    let mut out: Vec<(f64, String)> = Vec::new();
    for sent in sentences(text) {
        if sent.chars().count() > 400 {
            continue;
        }
        let Some(raise) = p.raise.find(&sent) else { continue };
        if !p.round.is_match(&sent) {
            continue;
        }
        // "plans to raise", "in talks to raise", "will raise" — money the
        // company has not received is not a round, and the catalogue was
        // adding it up with the real ones.
        let lead: String = sent[..raise.start()].to_lowercase();
        // Matched on a word boundary — a leading space before the trimmed
        // lead, so the cue "will" ends the phrase " ... will" and does not
        // fire on "Twill". "in talks" may sit mid-clause, so it stays a
        // contains-check.
        let lead_tail = format!(" {}", lead.trim_end());
        if ["plans to", "planning to", "plan to", "seeking to", "seeks to",
            "in talks to", "aims to", "aiming to", "will", "hopes to",
            "intends to", "expected to", "reportedly"]
            .iter()
            .any(|w| lead_tail.ends_with(&format!(" {w}")) || lead.contains("in talks"))
        {
            continue;
        }
        if let Some(no) = p.not_a_round.find(&sent) {
            // A sentence can carry both — "raised $500m at a $14b valuation".
            // Keep it only if the raising verb comes first.
            if no.start() < raise.start() {
                continue;
            }
        }
        // The round is the first figure after the verb of raising and before
        // any word that turns the sentence into a valuation. Taking the
        // largest instead read "raised $110 billion at a $730 billion
        // valuation" as a seven-hundred-billion-dollar round.
        let verb = raise.start();
        let stop = p
            .not_a_round
            .find_iter(&sent)
            .find(|m| m.start() > verb)
            .map(|m| m.start())
            .unwrap_or(sent.len());
        for m in p.money.captures_iter(&sent) {
            let whole = m.get(0).unwrap();
            if whole.start() < verb || whole.start() > stop {
                continue;
            }
            // "at a $35 billion valuation": the figure sits BEFORE the word
            // that damns it, so the stop above cannot see it. The figure is
            // a valuation when that word follows it with no other money in
            // between — "raised $110bn at a $730bn valuation" keeps its
            // $110bn, because the $730bn stands between it and the word.
            // A figure is a valuation only when a valuation word sits right
            // against it: "$X valuation"/"$X, valuing" (word just after, no
            // money between) or "valuation of $X"/"valued at $X" (word just
            // before). A valuation word further off in the sentence describes
            // a DIFFERENT figure — "raised $500M ... at a valuation of $3B"
            // keeps the $500M and skips the $3B — which the old
            // within-30-chars test got backwards, dropping the real round.
            let after = &sent[whole.end()..];
            let window: String = after.chars().take(16).collect::<String>().to_lowercase();
            let word_after = p.not_a_round.find(&window).map(|m| m.start());
            let money_after = p.money.find(&window).map(|m| m.start());
            let valued_after = matches!((word_after, money_after),
                (Some(w), m) if m.map(|mm| w < mm).unwrap_or(true));
            let before: String = sent[..whole.start()]
                .chars().rev().take(16).collect::<String>()
                .chars().rev().collect::<String>()
                .to_lowercase();
            let valued_before = before.contains("valuation of")
                || before.contains("valued at")
                || before.trim_end().ends_with("valuation")
                || before.trim_end().ends_with("worth");
            if valued_after || valued_before {
                continue;
            }
            let amount: f64 = m
                .get(1)
                .unwrap()
                .as_str()
                .replace(',', "")
                .parse()
                .unwrap_or(0.0);
            let mut value = amount * scale_of(m.get(2).map(|u| u.as_str()));
            // "€600 million ($645 million)" — the article converts for us,
            // and a catalogue in dollars should take the dollars.
            let first = whole.as_str().trim_start().chars().next().unwrap_or(' ');
            if first == '€' || first == '£' {
                let mut window_end = (whole.end() + 34).min(sent.len());
                while window_end < sent.len() && !sent.is_char_boundary(window_end) {
                    window_end += 1;
                }
                if let Some(next) = p
                    .money
                    .captures_iter(&sent[whole.end()..window_end])
                    .next()
                {
                    let nw = next.get(0).unwrap().as_str().trim_start();
                    if nw.starts_with('$') || nw.starts_with("US$") {
                        value = next
                            .get(1)
                            .unwrap()
                            .as_str()
                            .replace(',', "")
                            .parse::<f64>()
                            .unwrap_or(0.0)
                            * scale_of(next.get(2).map(|u| u.as_str()));
                    }
                }
            }
            // A euro or pound figure the article never converts is not a
            // dollar figure, and writing it as one moved a round by the
            // exchange rate. Inventing our own rate would be a different
            // lie, so the figure is passed over instead.
            let first = whole.as_str().trim_start().chars().next().unwrap_or(' ');
            if (first == '€' || first == '£') && value == amount * scale_of(m.get(2).map(|u| u.as_str())) {
                let converted_nearby = {
                    let we = (whole.end()..=(whole.end() + 34).min(sent.len()))
                        .rev()
                        .find(|i| sent.is_char_boundary(*i))
                        .unwrap_or(whole.end());
                    sent[whole.end()..we].contains('$')
                };
                if !converted_nearby {
                    continue;
                }
            }
            if value >= 100_000.0 {
                out.push((value, p.space.replace_all(&sent, " ").trim().to_string()));
            }
            break;
        }
    }
    // The same round is often stated twice in one article, in different
    // words. Dedup on the amount alone, not on the sentence text: keying on
    // the first 60 characters let a round restated with a different opening
    // ("Acme raised $50M ..." and "The round, in which Acme secured $50M ...")
    // count twice and double the company's total. Two genuinely distinct
    // rounds of the identical amount in one article are vanishingly rare, and
    // under-counting one is better than inventing a second.
    let mut seen: Vec<i64> = Vec::new();
    let mut kept = Vec::new();
    for (value, sent) in out {
        let key = value.round() as i64;
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        kept.push((value, sent));
    }
    kept
}

/// What Wikidata calls a company, a business, an enterprise, an organisation.
/// A page can sit in a category of AI companies and be about a bot, a product
/// or a market and only this distinguishes them.
pub const IS_A_COMPANY: &[&str] = &[
    "Q4830453", "Q783794", "Q6881511", "Q891723", "Q1058914", "Q167037",
    "Q43229", "Q18388277", "Q3918409",
];

/// Whether a search hit is the article about this company.
///
/// The title, less any disambiguator and any corporate suffix, must be the
/// name — not merely start with it. "OpenAI Five" matched OpenAI's article
/// and took its three hundred billion dollars.
pub fn title_matches(title: &str, want: &str) -> bool {
    same_company(&rp().aside.replace(title, ""), want)
}

/// The article is accepted only if it says it is a company and mentions the
/// field, which is what keeps a mountain range out.
pub fn article_is_about_a_company(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let low: String = text.chars().take(2500).collect::<String>().to_lowercase();
    if !low.contains("artificial intelligence") && !low.contains(" ai ") {
        return false;
    }
    rp().is_org.is_match(&low)
}

// ---------------------------------------------------------------------------
// Who put the money in
// ---------------------------------------------------------------------------

struct FundPats {
    names_investors: Regex,
    investor_first: Regex,
    stop: Regex,
    candidate: Regex,
    fund_word: Regex,
    aside: Regex,
    parts: Regex,
    non: Regex,
}

fn fp() -> &'static FundPats {
    static P: OnceLock<FundPats> = OnceLock::new();
    P.get_or_init(|| FundPats {
        names_investors: Regex::new(concat!(
            r"(?i)\b(?:led by|co-led by|backed by|from investors|investors includ\w+|",
            r"participation from|joined by|with investment from|investment from|",
            r"funded by|raised .{0,40}? from)\b",
        )).unwrap(),
        // The other shape the sentence takes: the investor is the subject,
        // not the object. Reading only the first shape left one company's
        // largest backer off its page while three smaller ones stood on it.
        investor_first: Regex::new(concat!(
            r"^\s*(?:(?:In|On|By|As of|Since)\s+[^,]{3,20},\s*(?:\d{4},\s*)?)?",
            r"([A-Z][\w.&'’-]*(?:\s+[A-Z][\w.&'’-]*){0,3})\s+",
            r"(?:announced|made|led|committed|invested|agreed)\b[^.]{0,90}?",
            r"\b(?:invest\w*|funding|round|stake)\b",
        )).unwrap(),
        stop: Regex::new(r"(?i)(?:[.;:]|\bat a\b|\bvaluing\b|\bwhich\b|\bvaluation\b)").unwrap(),
        candidate: Regex::new(
            r"^\s*([A-Z][\w.&'’-]*(?:\s+(?:[A-Z][\w.&'’-]*|de|van|of|and|&)){0,4})",
        ).unwrap(),
        fund_word: Regex::new(
            r"(?i)\b(capital|ventures?|partners?|fund|invest\w*|equity|growth|holdings?|management|associates|labs?|collective|angels?)\b",
        ).unwrap(),
        aside: Regex::new(r"\s*\([^)]*\)").unwrap(),
        parts: Regex::new(r",|\band\b|&").unwrap(),
        non: Regex::new(r"[^a-z0-9]+").unwrap(),
    })
}

/// Words that look like a name and are not one.
const NOT_A_FUND: &[&str] = &[
    "the", "a", "an", "series", "seed", "round", "million", "billion", "and",
    "others", "other", "existing", "new", "several", "various", "among",
    "including", "additional", "its", "their", "his", "her", "company",
    "january", "february", "march", "april", "may", "june", "july", "august",
    "september", "october", "november", "december", "us", "usd", "eur",
];

/// A fund's name almost always ends in one of a handful of words, or is a
/// famous exception.
const KNOWN_FUNDS: &[&str] = &[
    "andreessen horowitz", "a16z", "sequoia", "benchmark", "greylock", "kleiner perkins",
    "accel", "index", "lightspeed", "founders fund", "khosla", "thrive", "coatue",
    "tiger global", "general catalyst", "insight", "bessemer", "iconiq", "battery",
    "y combinator", "softbank", "temasek", "gic", "mubadala", "qia", "nvidia",
    "microsoft", "google", "alphabet", "amazon", "salesforce", "intel", "samsung",
    "nea", "menlo", "redpoint", "spark", "crv", "matrix", "dst global", "b capital",
    "eurazeo", "balderton", "atomico", "northzone", "hv capital", "point nine",
    "sapphire", "scale venture", "sound ventures", "conviction", "radical ventures",
];

pub fn looks_like_a_fund(name: &str) -> bool {
    let low = name.to_lowercase();
    let low = low.trim();
    if low.chars().count() < 3 || NOT_A_FUND.contains(&low) {
        return false;
    }
    if low
        .split_whitespace()
        .next()
        .is_some_and(|w| NOT_A_FUND.contains(&w))
    {
        return false;
    }
    fp().fund_word.is_match(low) || KNOWN_FUNDS.contains(&low)
}

/// The funds a sentence names as having paid.
///
/// The clause is cut into its members first — on commas and on "and" — and
/// each tested on its own. Matching a capitalised run across the whole clause
/// instead produced "Bessemer Venture Partners and General", which is two
/// funds welded into one that does not exist.
pub fn investors_in(sentence: &str) -> Vec<String> {
    let p = fp();
    let mut out: Vec<String> = Vec::new();
    if let Some(c) = p.investor_first.captures(sentence) {
        let lead = c.get(1).unwrap().as_str().trim().to_string();
        if looks_like_a_fund(&lead) {
            out.push(lead);
        }
    }
    for m in p.names_investors.find_iter(sentence) {
        let rest = &sentence[m.end()..];
        let clause = match p.stop.find(rest) {
            Some(e) => &rest[..e.start()],
            None => rest,
        };
        for part in p.parts.split(clause) {
            let part = part.trim_matches([' ', '.', ',', ';', '(', ')']);
            // A parenthetical amount rides along with a name in lists of the
            // form "Amazon ($50 billion), SoftBank ($30 billion)".
            let part = p.aside.replace_all(part, "");
            let part = part.trim();
            let Some(c) = p.candidate.captures(part) else { continue };
            let name = c
                .get(1)
                .unwrap()
                .as_str()
                .trim_matches([' ', ',', '.', '&'])
                .to_string();
            if looks_like_a_fund(&name) {
                out.push(name);
            }
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut kept = Vec::new();
    for n in out {
        let k = n.to_lowercase();
        if !seen.contains(&k) {
            seen.push(k);
            kept.push(n);
        }
    }
    kept
}

pub fn fund_id(name: &str) -> String {
    format!(
        "fund_{}",
        fp().non
            .replace_all(&name.to_lowercase(), "-")
            .trim_matches('-')
    )
}

#[cfg(test)]
mod round_tests {
    use super::*;

    /// A thousands-grouped amount is read whole, not truncated at the first
    /// comma group — the bug that read $110,000,000 as $110,000.
    #[test]
    fn a_grouped_amount_is_read_whole() {
        let got = rounds_in("The company raised $110,000,000 in a funding round.");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 110_000_000.0);
    }

    /// The mistake the rule exists to prevent: a valuation is not money
    /// received.
    #[test]
    fn a_valuation_is_not_a_round() {
        // A sentence with no round word ("funding", "round", "series")
        // yields nothing by design; the real newspaper sentence carries one.
        let got = rounds_in(
            "OpenAI raised $110 billion in a funding round at a $730 billion valuation.",
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 110e9);

        assert!(rounds_in("The company was valued at $14 billion in a funding round.").is_empty());
    }

    /// The ordinary English order — "a round at a $X valuation" — carries
    /// its only figure before the word that damns it.
    #[test]
    fn a_sole_figure_before_the_valuation_word_is_a_valuation() {
        assert!(rounds_in("The company raised new funding at a $35 billion valuation.").is_empty());
    }

    /// Money not yet received is not a round.
    #[test]
    fn a_planned_round_is_not_a_round() {
        assert!(rounds_in("The startup plans to raise $500 million in a new round.").is_empty());
        assert!(rounds_in("It is in talks to raise $2 billion in funding.").is_empty());
    }

    /// A foreign figure the article never converts stays out rather than
    /// being written down as dollars.
    #[test]
    fn an_unconverted_euro_figure_is_passed_over() {
        assert!(rounds_in("Mistral raised €600 million in a Series B round.").is_empty());
    }

    /// The article converts for us, and a catalogue in dollars takes dollars.
    #[test]
    fn a_converted_figure_is_read_in_dollars() {
        let got = rounds_in("Mistral raised €600 million ($645 million) in a Series B round.");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, 645e6);
    }

    /// Two funds in a list are two funds, not one welded name.
    #[test]
    fn a_list_of_investors_is_cut_before_it_is_read() {
        let got = investors_in(
            "The round was led by Bessemer Venture Partners and General Catalyst, \
             with participation from Index Ventures.",
        );
        assert_eq!(got, vec!["Bessemer Venture Partners", "General Catalyst", "Index Ventures"]);
    }

    #[test]
    fn an_investor_can_be_the_subject_of_the_sentence() {
        let got = investors_in(
            "In 2023, Microsoft announced an additional $10 billion investment in OpenAI.",
        );
        assert_eq!(got, vec!["Microsoft"]);
    }
}

// ---------------------------------------------------------------------------
// Reading the prose off Wikipedia
// ---------------------------------------------------------------------------

/// Percent-encoding as `urllib.parse.quote` does it: everything outside the
/// unreserved set and the path separator is escaped, so the source URL a page
/// shows is the one the fund reader can turn back into a title.
pub fn quote_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '~' | '/') {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct Wiki {
    http: reqwest::Client,
}

impl Wiki {
    pub fn new() -> Result<Self> {
        Ok(Self {
            // Python's urlopen counts its forty seconds per read, so a slow
            // article still arrives; a whole-request budget of forty dropped
            // five companies on a busy night and their investors with them.
            http: reqwest::Client::builder()
                .user_agent(PROSE_UA)
                .connect_timeout(std::time::Duration::from_secs(20))
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
        })
    }

    async fn api(&self, params: &[(&str, &str)]) -> Option<serde_json::Value> {
        self.http
            .get(WIKI)
            .query(params)
            .send()
            .await
            .ok()?
            .json::<serde_json::Value>()
            .await
            .ok()
    }

    /// The article about this company, and its text, or nothing.
    ///
    /// Searched rather than guessed, because "Cohere" is a word and "Sierra"
    /// is a mountain range.
    pub async fn article(&self, name: &str) -> (Option<String>, String) {
        let query = format!("{name} artificial intelligence company");
        let Some(d) = self
            .api(&[
                ("action", "query"),
                ("list", "search"),
                ("format", "json"),
                ("srsearch", &query),
                ("srlimit", "3"),
            ])
            .await
        else {
            return (None, String::new());
        };
        let hits: Vec<String> = d["query"]["search"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|h| h["title"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for title in hits {
            if !title_matches(&title, name) {
                continue;
            }
            let Some(page) = self
                .api(&[
                    ("action", "query"),
                    ("prop", "extracts"),
                    ("explaintext", "1"),
                    ("format", "json"),
                    ("redirects", "1"),
                    ("titles", &title),
                ])
                .await
            else {
                continue;
            };
            let text = page["query"]["pages"]
                .as_object()
                .and_then(|m| m.values().next())
                .and_then(|p| p["extract"].as_str())
                .unwrap_or("")
                .to_string();
            if !article_is_about_a_company(&text) {
                continue;
            }
            return (Some(title), text);
        }
        (None, String::new())
    }

    /// What Wikidata records about the subject of a Wikipedia article.
    ///
    /// The article is asked for its own Wikidata id rather than the name
    /// being searched for: matching by label failed on every company filed
    /// under a disambiguated title — Harvey (software), Cursor (code editor)
    /// — and those are exactly the young companies this is for.
    pub async fn claims(&self, title: &str) -> Option<serde_json::Value> {
        let d = self
            .api(&[
                ("action", "query"),
                ("prop", "pageprops"),
                ("format", "json"),
                ("redirects", "1"),
                ("titles", title),
            ])
            .await?;
        let qid = d["query"]["pages"]
            .as_object()?
            .values()
            .next()?
            .get("pageprops")?
            .get("wikibase_item")?
            .as_str()?
            .to_string();
        let e: serde_json::Value = self
            .http
            .get(format!(
                "https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"
            ))
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        e["entities"][&qid]["claims"].as_object().map(|_| {
            e["entities"][&qid]["claims"].clone()
        })
    }
}

/// Whether the company trades on a public exchange.
///
/// A public company once took venture money and is no longer a startup: it
/// now runs on the public's money and answers to it. P414 stock exchange,
/// P946 ISIN — either says it is listed.
pub fn is_listed(claims: Option<&serde_json::Value>) -> bool {
    claims.is_some_and(|c| !c["P414"].is_null() || !c["P946"].is_null())
}

pub fn is_company(claims: Option<&serde_json::Value>) -> bool {
    let Some(c) = claims else { return false };
    let Some(p31) = c["P31"].as_array() else { return false };
    p31.iter().any(|s| {
        s["mainsnak"]["datavalue"]["value"]["id"]
            .as_str()
            .is_some_and(|id| IS_A_COMPANY.contains(&id))
    })
}

/// The columns the round reader fills. A company with no round we could read
/// keeps them empty, which is a different sentence from "took no money".
const STARTUP_COLUMNS: &[&str] = &[
    "ALTER TABLE providers ADD COLUMN listed INTEGER",
    "ALTER TABLE providers ADD COLUMN raised INTEGER",
    "ALTER TABLE providers ADD COLUMN rounds INTEGER",
    "ALTER TABLE providers ADD COLUMN raised_source TEXT",
    "ALTER TABLE providers ADD COLUMN founded TEXT",
];

pub struct Raised {
    pub id: String,
    pub name: String,
    pub total: i64,
    pub rounds: usize,
    pub url: String,
    pub first: String,
}

/// Every company here whose rounds we have not yet read, read.
pub async fn read_rounds(
    con: &rusqlite::Connection,
    limit: usize,
) -> Result<(usize, Vec<Raised>, Vec<String>)> {
    for stmt in STARTUP_COLUMNS {
        let _ = con.execute(stmt, []);
    }
    let mut q = con.prepare(
        "SELECT id, name FROM providers WHERE raised IS NULL ORDER BY \
         (SELECT COUNT(*) FROM offerings o WHERE o.provider_id = providers.id) DESC",
    )?;
    let mut todo: Vec<(String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if limit > 0 {
        todo.truncate(limit);
    }
    let read = todo.len();

    let wiki = Wiki::new()?;
    let (mut found, mut dry) = (Vec::new(), Vec::new());
    for (pid, name) in todo {
        let (Some(title), text) = wiki.article(&name).await else {
            dry.push(name);
            continue;
        };
        let rs = rounds_in(&text);
        if rs.is_empty() {
            dry.push(name);
            continue;
        }
        let claims = wiki.claims(&title).await;
        if is_listed(claims.as_ref()) || !is_company(claims.as_ref()) {
            dry.push(format!("{name} (not a private company)"));
            continue;
        }
        let total: f64 = rs.iter().map(|(v, _)| v).sum();
        found.push(Raised {
            id: pid,
            name,
            total: total.round() as i64,
            rounds: rs.len(),
            url: format!(
                "https://en.wikipedia.org/wiki/{}",
                quote_path(&title.replace(' ', "_"))
            ),
            first: rs[0].1.chars().take(110).collect(),
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    Ok((read, found, dry))
}

pub fn write_rounds(con: &rusqlite::Connection, found: &[Raised]) -> Result<()> {
    for f in found {
        con.execute(
            "UPDATE providers SET raised=?1, rounds=?2, raised_source=?3, listed=0 WHERE id=?4",
            rusqlite::params![f.total, f.rounds as i64, f.url, f.id],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// A fund is a company, and an investment is an edge between two of them
// ---------------------------------------------------------------------------

const INVESTMENT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS investments (
    fund_id    TEXT NOT NULL REFERENCES providers(id),
    company_id TEXT NOT NULL REFERENCES providers(id),
    source_url TEXT NOT NULL,
    UNIQUE (fund_id, company_id)
);
CREATE INDEX IF NOT EXISTS inv_by_fund ON investments(fund_id);
CREATE INDEX IF NOT EXISTS inv_by_company ON investments(company_id);
";

/// Insertion order decides two things — the spelling a fund is filed under
/// and, where a fund is named twice for one company, which source is kept —
/// so the tables here are ordered rather than hashed.
#[derive(Default)]
pub struct Ledger {
    /// lower-cased fund name -> the spelling first seen
    pub funds: Vec<(String, String)>,
    /// (lower-cased fund name, company id) -> source
    pub edges: Vec<((String, String), String)>,
    pub yc: usize,
    pub read: usize,
}

impl Ledger {
    fn add(&mut self, fund: &str, cid: &str, src: &str) {
        let key = fund.to_lowercase();
        if !self.funds.iter().any(|(k, _)| *k == key) {
            self.funds.push((key.clone(), fund.to_string()));
        }
        let at = (key, cid.to_string());
        match self.edges.iter_mut().find(|(k, _)| *k == at) {
            Some(e) => e.1 = src.to_string(),
            None => self.edges.push((at, src.to_string())),
        }
    }
}

pub async fn read_investors(con: &rusqlite::Connection) -> Result<Ledger> {
    con.execute_batch(INVESTMENT_SCHEMA)?;
    let mut led = Ledger::default();

    // Y Combinator says so itself, of every company in its portfolio.
    let mut q = con.prepare(
        "SELECT id, backing FROM providers WHERE backing LIKE 'Y Combinator%'",
    )?;
    let yc: Vec<String> = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    for pid in yc {
        led.add("Y Combinator", &pid, "https://www.ycombinator.com/companies");
        led.yc += 1;
    }

    // And the round sentences name the rest.
    let mut q = con.prepare(
        "SELECT id, name, raised_source FROM providers \
         WHERE raised_source IS NOT NULL AND raised_source <> ''",
    )?;
    let sourced: Vec<(String, String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    let wiki = Wiki::new()?;
    let aside = Regex::new(r"\s*\([^)]*\)\s*$").unwrap();
    for (pid, _name, src) in sourced {
        let title = unquote(src.rsplit('/').next().unwrap_or("")).replace('_', " ");
        let (_, text) = wiki.article(aside.replace(&title, "").trim()).await;
        if text.is_empty() {
            continue;
        }
        led.read += 1;
        for (_, sentence) in rounds_in(&text) {
            for who in investors_in(&sentence) {
                led.add(&who, &pid, &src);
            }
        }
    }
    Ok(led)
}

/// A fund is a company. Where one is already here under its own name, the
/// edge points at that row rather than making a second one.
pub fn write_investors(con: &rusqlite::Connection, led: &Ledger) -> Result<usize> {
    let mut q = con.prepare("SELECT id, name FROM providers")?;
    let providers: Vec<(String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    let mut made = 0;
    let mut ids: Vec<(String, String)> = Vec::new();
    for (key, spelling) in &led.funds {
        match providers.iter().find(|(_, n)| same_company(spelling, n)) {
            Some((i, _)) => ids.push((key.clone(), i.clone())),
            None => {
                let fid = fund_id(spelling);
                con.execute(
                    "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,'','fund')",
                    rusqlite::params![fid, spelling],
                )?;
                ids.push((key.clone(), fid));
                made += 1;
            }
        }
    }
    for ((key, cid), src) in &led.edges {
        let fid = &ids.iter().find(|(k, _)| k == key).unwrap().1;
        if fid == cid {
            continue; // a fund does not invest in itself
        }
        con.execute(
            "INSERT OR IGNORE INTO investments (fund_id, company_id, source_url) \
             VALUES (?1,?2,?3)",
            rusqlite::params![fid, cid, src],
        )?;
    }
    Ok(made)
}

/// Thousands separated, as the reports have always printed money.
pub fn with_commas(n: i64) -> String {
    let d = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

// ---------------------------------------------------------------------------
// Everything findable about one fund's portfolio, gathered in one pass
// ---------------------------------------------------------------------------

pub const SITE_UA: &str = "Mozilla/5.0 (compatible; pass-index/1.0; +https://pass.io)";
pub const YC_ALL: &str = "https://yc-oss.github.io/api/companies/all.json";

struct SitePats {
    empty: Regex,
    parked: Regex,
    title: Regex,
    desc: Regex,
    og_desc: Regex,
    space: Regex,
    non: Regex,
    non_space: Regex,
    para: Regex,
}

fn sp() -> &'static SitePats {
    static P: OnceLock<SitePats> = OnceLock::new();
    P.get_or_init(|| SitePats {
        // A description worth keeping says something; these say nothing.
        empty: Regex::new(concat!(
            r"(?i)^\s*(home|homepage|welcome|index|coming soon|untitled|",
            r"page not found|404|just a moment)\b",
        )).unwrap(),
        // A guessed domain is often not the company's: it is for sale, or it
        // belongs to somebody with a similar name. Both wrote nonsense onto a
        // card — "Sutro.ai is for sale on Spaceship", and OneSpan's own copy
        // onto VASCO — so a reading must survive all three tests to be kept.
        parked: Regex::new(concat!(
            r"(?i)\b(is (available )?for sale|buy this domain|domain (is )?for sale|",
            r"parked|godaddy|sedo|namecheap|hugedomains|spaceship|",
            // The lander copy the domain marketplaces actually serve.
            // Without these, "Own this domain today" and "Get set up with a
            // new domain name right away" went onto company cards.
            r"own this domain|premium \.?com domain|domain name right away|",
            r"domain you deserve|payment plans (to fit|available)|",
            r"find information, resources|register this)\b",
        )).unwrap(),
        title: Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap(),
        desc: Regex::new(
            r#"(?is)<meta[^>]+name=["']description["'][^>]+content=["'](.*?)["']"#,
        ).unwrap(),
        og_desc: Regex::new(
            r#"(?is)<meta[^>]+property=["']og:description["'][^>]+content=["'](.*?)["']"#,
        ).unwrap(),
        space: Regex::new(r"\s+").unwrap(),
        non: Regex::new(r"[^a-z0-9]").unwrap(),
        non_space: Regex::new(r"[^a-z0-9 ]").unwrap(),
        para: Regex::new(r"\n\n").unwrap(),
    })
}

fn head(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub fn bare_name(name: &str) -> String {
    sp().non.replace_all(&name.to_lowercase(), "").into_owned()
}

/// Whether a page is about this company.
///
/// The address counts as evidence only when somebody chose it — the URL the
/// catalogue already held for the company. A guessed address carries the
/// name by construction, so counting it made the test pass for every
/// reading ever put to it, and pages that never mention the company went
/// onto its card.
pub fn worth_keeping(name: &str, text: &str, chosen_src: &str) -> bool {
    let p = sp();
    if text.trim().chars().count() < 30 || p.parked.is_match(text) {
        return false;
    }
    let key = bare_name(name);
    let blob = bare_name(&format!("{text}{chosen_src}"));
    // The page must carry the company's full bare name. A 6-character prefix
    // let any page whose copy held a common prefix ("context…" for Contextual
    // AI) pass, so a parked or unrelated lander became the description. The
    // one allowance is a trailing "ai" the page dropped — "Perplexity" for
    // "Perplexity AI" — which is the name itself, not a prefix of it.
    if key.is_empty() || blob.contains(&key) {
        return true;
    }
    let stem = key.strip_suffix("ai").unwrap_or(&key);
    stem.len() >= 4 && blob.contains(stem)
}

/// The addresses a company of this name most likely answers on.
///
/// The order is the order they are written here. The Python this replaces
/// held the stems in a set, so which eight of the sixteen candidates got
/// tried changed from one run to the next with the interpreter's string
/// hashing — the same company could be read off its own site one night and
/// off nothing the next.
pub fn guess_site(name: &str) -> Vec<String> {
    let p = sp();
    let low = name.to_lowercase();
    let base = p.non.replace_all(&low, "").into_owned();
    let cleaned = p.non_space.replace_all(&low, "").into_owned();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let mut stems: Vec<String> = vec![base.clone()];
    if words.len() > 1 {
        for s in [words.concat(), words.join("-")] {
            if !stems.contains(&s) {
                stems.push(s);
            }
        }
    }
    if base.ends_with("ai") && base.chars().count() > 4 {
        let s = base[..base.len() - 2].to_string();
        if !stems.contains(&s) {
            stems.push(s);
        }
    }
    let mut out = Vec::new();
    for s in stems {
        if s.is_empty() {
            continue;
        }
        out.push(format!("https://{s}.ai"));
        out.push(format!("https://{s}.com"));
        out.push(format!("https://www.{s}.com"));
        out.push(format!("https://{s}.io"));
    }
    out.truncate(8);
    out
}

pub struct Site {
    pub title: String,
    pub desc: String,
}

impl Site {
    /// The description if the page has one, and its title otherwise.
    pub fn says(&self) -> &str {
        if self.desc.is_empty() { &self.title } else { &self.desc }
    }
}

pub struct Sites {
    http: reqwest::Client,
}

impl Sites {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .user_agent(SITE_UA)
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
        })
    }

    async fn fetch(&self, url: &str) -> Option<String> {
        let body = self.http.get(url).send().await.ok()?.bytes().await.ok()?;
        let cut = body.len().min(200_000);
        Some(String::from_utf8_lossy(&body[..cut]).into_owned())
    }

    /// What a page says it is: its title and its own description.
    pub async fn read(&self, url: &str) -> Option<Site> {
        let p = sp();
        let html = self.fetch(url).await?;
        let grab = |re: &Regex| -> String {
            re.captures(&html)
                .map(|c| p.space.replace_all(c.get(1).unwrap().as_str(), " ").trim().to_string())
                .unwrap_or_default()
        };
        let title = grab(&p.title);
        let mut desc = grab(&p.desc);
        if desc.is_empty() {
            desc = grab(&p.og_desc);
        }
        if p.empty.is_match(&desc) || desc.chars().count() < 25 {
            desc = String::new();
        }
        let (title, desc) = (head(&title, 160), head(&desc, 600));
        if title.is_empty() && desc.is_empty() {
            return None;
        }
        Some(Site { title, desc })
    }

    pub async fn yc_directory(&self) -> Result<Vec<(String, serde_json::Value)>, String> {
        let body = self
            .http
            .get(YC_ALL)
            .timeout(std::time::Duration::from_secs(90))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| e.to_string())?;
        Ok(body
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| (bare_name(c["name"].as_str().unwrap_or("")), c.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// One description per subject, and only if it says something.
///
/// Three collectors write descriptions and each used to add its own beside
/// whatever was already there, so 400 things held two and the card printed
/// whichever the query returned first. The newest reading wins, and a reading
/// under forty characters is a title or a placeholder and is not a reading.
pub fn write_description(
    con: &rusqlite::Connection,
    subject: &str,
    text: &str,
    source: &str,
) -> Result<bool> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() < 40 {
        return Ok(false);
    }
    con.execute(
        "DELETE FROM docs WHERE subject=?1 AND kind='description'",
        [subject],
    )?;
    con.execute(
        "INSERT INTO docs (subject,kind,field,text,source_url,taken_at) \
         VALUES (?1,'description','',?2,?3,date('now'))",
        rusqlite::params![subject, head(&text, 900), source],
    )?;
    Ok(true)
}

pub struct Blurb {
    pub id: String,
    pub text: String,
    pub url: String,
    pub source: String,
}

pub struct Gathered {
    pub portfolio: usize,
    /// In the order the sources are tried, cheapest first.
    pub tally: Vec<(&'static str, usize)>,
    pub writes: Vec<Blurb>,
    pub note: Option<String>,
}

/// Four sources, cheapest first, and each stops when it has an answer.
pub async fn gather(con: &rusqlite::Connection, fund: &str) -> Result<Option<Gathered>> {
    let fid: Option<String> = con
        .query_row("SELECT id FROM providers WHERE name=?1", [fund], |r| r.get(0))
        .ok();
    let Some(fid) = fid else { return Ok(None) };

    let mut q = con.prepare(
        "SELECT p.id, p.name, COALESCE(p.url,'') FROM investments i \
         JOIN providers p ON p.id = i.company_id WHERE i.fund_id = ?1 ORDER BY p.name",
    )?;
    let rows: Vec<(String, String, String)> = q
        .query_map([fid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    // A company whose stored description reads as a domain-sale lander is
    // treated as having none: the bad reading was permanent otherwise —
    // "already here" skipped it every night from then on.
    let mut q = con.prepare("SELECT subject, text FROM docs WHERE kind='description'")?;
    let has_desc: Vec<String> = q
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()?
        .into_iter()
        .filter(|(_, t)| !sp().parked.is_match(t))
        .map(|(s, _)| s)
        .collect();
    drop(q);

    let sites = Sites::new()?;
    let (yc, note) = match sites.yc_directory().await {
        Ok(y) => (y, None),
        Err(e) => (Vec::new(), Some(format!("  Y Combinator directory unavailable: {e}"))),
    };

    let wiki = Wiki::new()?;
    let mut tally = vec![
        ("already here", 0usize),
        ("from Y Combinator", 0),
        ("from its own site", 0),
        ("from Wikipedia", 0),
        ("nothing found", 0),
    ];
    let mut bump = |k: &str, tally: &mut Vec<(&'static str, usize)>| {
        tally.iter_mut().find(|(n, _)| *n == k).unwrap().1 += 1;
    };
    let mut writes: Vec<Blurb> = Vec::new();

    for (pid, name, url) in &rows {
        let mut url = url.clone();
        if has_desc.contains(pid) && !url.is_empty() {
            bump("already here", &mut tally);
            continue;
        }
        let key = bare_name(name);
        if let Some((_, c)) = yc.iter().find(|(k, _)| *k == key) {
            let long = c["long_description"].as_str().unwrap_or("");
            let one = c["one_liner"].as_str().unwrap_or("");
            if !long.is_empty() || !one.is_empty() {
                let text = if long.is_empty() { one } else { long };
                writes.push(Blurb {
                    id: pid.clone(),
                    text: head(text, 900),
                    url: c["website"].as_str().unwrap_or("").trim().to_string(),
                    source: format!(
                        "https://www.ycombinator.com/companies/{}",
                        c["slug"].as_str().unwrap_or("")
                    ),
                });
                bump("from Y Combinator", &mut tally);
                continue;
            }
        }
        let mut site = None;
        let chosen = url.clone();
        let first: Vec<String> = if url.is_empty() { vec![] } else { vec![url.clone()] };
        for cand in first.into_iter().chain(guess_site(name)) {
            if let Some(s) = sites.read(&cand).await {
                url = cand;
                site = Some(s);
                break;
            }
        }
        if let Some(s) = &site {
            let evidence = if url == chosen { chosen.as_str() } else { "" };
            if worth_keeping(name, s.says(), evidence) {
                writes.push(Blurb {
                    id: pid.clone(),
                    text: s.says().to_string(),
                    url: url.clone(),
                    source: url.clone(),
                });
                bump("from its own site", &mut tally);
                continue;
            }
        }
        let (title, text) = wiki.article(name).await;
        if let Some(title) = title.filter(|_| !text.is_empty()) {
            let p = sp();
            let first_para = p.para.split(text.trim()).next().unwrap_or("").to_string();
            writes.push(Blurb {
                id: pid.clone(),
                text: head(p.space.replace_all(&first_para, " ").as_ref(), 600),
                url: url.clone(),
                source: format!(
                    "https://en.wikipedia.org/wiki/{}",
                    quote_path(&title.replace(' ', "_"))
                ),
            });
            bump("from Wikipedia", &mut tally);
            continue;
        }
        bump("nothing found", &mut tally);
    }
    Ok(Some(Gathered { portfolio: rows.len(), tally, writes, note }))
}

pub fn write_blurbs(con: &rusqlite::Connection, writes: &[Blurb]) -> Result<()> {
    for w in writes {
        write_description(con, &w.id, &w.text, &w.source)?;
        if !w.url.is_empty() {
            con.execute(
                "UPDATE providers SET url=?1 WHERE id=?2 AND COALESCE(url,'')=''",
                rusqlite::params![w.url, w.id],
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Y Combinator's own directory, which is a thousand funded AI companies
// ---------------------------------------------------------------------------

/// The Y Combinator directory, mirrored as JSON by yc-oss from YC's own API.
const YC_TAGS: &[&str] = &[
    "artificial-intelligence", "generative-ai", "ai-assistant", "aiops",
    "ai-powered-drug-discovery", "machine-learning", "conversational-ai",
    "computer-vision", "nlp", "ml",
];

const BACKING_COLUMNS: &[&str] = &[
    "ALTER TABLE providers ADD COLUMN backing TEXT",
    "ALTER TABLE providers ADD COLUMN status TEXT",
];

pub struct Batch {
    /// Companies already here, and the batch that admitted them.
    pub matched: Vec<(String, serde_json::Value)>,
    pub fresh: Vec<serde_json::Value>,
    pub listed: usize,
    pub dead: usize,
}

impl Sites {
    /// Every company under any of the tags, once each.
    pub async fn yc_tags(&self) -> (Vec<serde_json::Value>, Vec<String>) {
        let mut out: Vec<(i64, serde_json::Value)> = Vec::new();
        let mut notes = Vec::new();
        for tag in YC_TAGS {
            let url = format!("https://yc-oss.github.io/api/tags/{tag}.json");
            let rows = match self
                .http
                .get(&url)
                .timeout(std::time::Duration::from_secs(90))
                .send()
                .await
            {
                Ok(r) => match r.json::<serde_json::Value>().await {
                    Ok(v) => v,
                    Err(e) => { notes.push(format!("  {tag}: {e}")); continue }
                },
                Err(e) => { notes.push(format!("  {tag}: {e}")); continue }
            };
            for c in rows.as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let id = c["id"].as_i64().unwrap_or(-1);
                if !out.iter().any(|(k, _)| *k == id) {
                    out.push((id, c.clone()));
                }
            }
        }
        (out.into_iter().map(|(_, c)| c).collect(), notes)
    }
}

/// Membership is itself the evidence: YC invests in every company it admits,
/// so the venture mark is earned by the source rather than by a sentence
/// somebody wrote about a round. Companies it lists as inactive stay out — a
/// dead company is a fact about the market's history, and this catalogue is
/// about what is sold.
pub async fn yc_batch(con: &rusqlite::Connection) -> Result<(Batch, Vec<String>)> {
    for stmt in BACKING_COLUMNS {
        let _ = con.execute(stmt, []);
    }
    let sites = Sites::new()?;
    let (rows, notes) = sites.yc_tags().await;

    let mut q = con.prepare("SELECT id, name FROM providers")?;
    let held: Vec<(String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    let mut b = Batch { matched: Vec::new(), fresh: Vec::new(), listed: rows.len(), dead: 0 };
    for c in rows {
        if c["status"].as_str() == Some("Inactive") {
            b.dead += 1;
            continue;
        }
        let name = c["name"].as_str().unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        match held.iter().find(|(_, h)| same_company(&name, h)) {
            Some((pid, _)) => b.matched.push((pid.clone(), c)),
            None => b.fresh.push(c),
        }
    }
    Ok((b, notes))
}

fn admitted_by(c: &serde_json::Value) -> String {
    format!("Y Combinator · {}", c["batch"].as_str().unwrap_or(""))
}

pub fn write_batch(con: &rusqlite::Connection, b: &Batch) -> Result<(usize, usize)> {
    let mut touched = 0;
    for (pid, c) in &b.matched {
        con.execute(
            "UPDATE providers SET backing=?1, status=?2 WHERE id=?3 AND backing IS NULL",
            rusqlite::params![admitted_by(c), c["status"].as_str(), pid],
        )?;
        touched += 1;
    }
    let mut added = 0;
    for c in &b.fresh {
        let name = c["name"].as_str().unwrap_or("").trim().to_string();
        let pid = provider_id(&name);
        let taken: bool = con
            .query_row("SELECT 1 FROM providers WHERE id=?1", [&pid], |_| Ok(()))
            .is_ok();
        if taken {
            continue;
        }
        con.execute(
            "INSERT INTO providers (id,name,url,kind,backing,status,listed) \
             VALUES (?1,?2,?3,'vendor',?4,?5,0)",
            rusqlite::params![
                pid,
                name,
                c["website"].as_str().unwrap_or("").trim(),
                admitted_by(c),
                c["status"].as_str()
            ],
        )?;
        let long = c["long_description"].as_str().unwrap_or("");
        let one = c["one_liner"].as_str().unwrap_or("");
        write_description(
            con,
            &pid,
            if long.is_empty() { one } else { long },
            &format!(
                "https://www.ycombinator.com/companies/{}",
                c["slug"].as_str().unwrap_or("")
            ),
        )?;
        added += 1;
    }
    Ok((added, touched))
}

// ---------------------------------------------------------------------------
// Companies in this market that the catalogue has never heard of
// ---------------------------------------------------------------------------

/// Where the encyclopedia keeps them. Categories rather than a search,
/// because a category is somebody's judgement that the article belongs and a
/// search is a string match.
const SOURCES: &[(&str, &str)] = &[
    ("Category:Artificial intelligence companies", "category"),
    ("Category:Generative AI companies", "category"),
    ("Category:Anthropic", "category"),
    ("Category:OpenAI", "category"),
    ("List of artificial intelligence companies", "list"),
];

fn not_a_company() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // An article about a market, a law or a person rather than a company.
    R.get_or_init(|| Regex::new(
        r"(?i)^(List of|Category:|Timeline|History of|Comparison of|Outline of|Artificial intelligence in|Regulation of)"
    ).unwrap())
}

fn disambiguator() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)\s*\((company|software|firm|startup|AI|business)\)\s*$").unwrap()
    })
}

impl Wiki {
    /// Every company name the sources offer, once each, keyed by the form
    /// the catalogue compares names in.
    pub async fn listed_companies(&self) -> (Vec<(String, String)>, Vec<String>) {
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut notes = Vec::new();
        for (title, how) in SOURCES {
            let got = if *how == "category" {
                self.api(&[
                    ("action", "query"), ("format", "json"),
                    ("list", "categorymembers"), ("cmtitle", title),
                    ("cmlimit", "500"), ("cmtype", "page"),
                ]).await.map(|d| {
                    d["query"]["categorymembers"].as_array().map(|a| {
                        a.iter().filter_map(|m| m["title"].as_str().map(str::to_string)).collect()
                    }).unwrap_or_default()
                })
            } else {
                self.api(&[
                    ("action", "query"), ("format", "json"),
                    ("prop", "links"), ("titles", title),
                    ("pllimit", "500"), ("plnamespace", "0"),
                ]).await.map(|d| {
                    let mut out: Vec<String> = Vec::new();
                    if let Some(pages) = d["query"]["pages"].as_object() {
                        for p in pages.values() {
                            for l in p["links"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                                if let Some(t) = l["title"].as_str() {
                                    out.push(t.to_string());
                                }
                            }
                        }
                    }
                    out
                })
            };
            let Some(found) = got else {
                notes.push(format!("  {title}: unavailable"));
                continue;
            };
            for n in found {
                if not_a_company().is_match(&n) {
                    continue;
                }
                let key = crate::resolve::norm(&n);
                if !seen.iter().any(|(k, _)| *k == key) {
                    seen.push((key, n));
                }
            }
        }
        (seen, notes)
    }
}

pub struct Unheard {
    pub name: String,
    pub total: i64,
    pub rounds: usize,
    pub url: String,
    pub blurb: String,
}

pub struct Sweep {
    pub offered: usize,
    pub fresh: usize,
    pub found: Vec<Unheard>,
    pub quiet: Vec<String>,
    pub notes: Vec<String>,
}

/// The collectors all start from a price: something is in here because
/// somebody sells it and published a rate. That misses every company whose
/// product is sold by conversation and every one whose product has not
/// shipped, which between them is most of the venture-funded field. This
/// starts from the other end — and only a company with a round we can read
/// lands, because a company nobody funded and nobody prices is a name.
pub async fn discover(con: &rusqlite::Connection, limit: usize) -> Result<Sweep> {
    for stmt in STARTUP_COLUMNS {
        let _ = con.execute(stmt, []);
    }
    let mut held: Vec<String> = Vec::new();
    for sql in [
        "SELECT name FROM providers",
        // A company can be here under a name the encyclopedia spells
        // differently.
        "SELECT alias FROM aliases",
    ] {
        let mut q = con.prepare(sql)?;
        for n in q.query_map([], |r| r.get::<_, String>(0))? {
            let k = crate::resolve::norm(&n?);
            if !held.contains(&k) {
                held.push(k);
            }
        }
    }

    let wiki = Wiki::new()?;
    let (offered, notes) = wiki.listed_companies().await;
    let mut todo: Vec<(String, String)> = offered
        .iter()
        .filter(|(k, _)| !held.contains(k))
        .cloned()
        .collect();
    let fresh = todo.len();
    todo.sort();
    if limit > 0 {
        todo.truncate(limit);
    }

    let (mut found, mut quiet) = (Vec::new(), Vec::new());
    for (_, name) in todo {
        // An article title carries a disambiguator the company does not.
        let clean = disambiguator().replace(&name, "").trim().to_string();
        let (Some(title), text) = wiki.article(&clean).await else {
            quiet.push(clean);
            continue;
        };
        // A category of AI companies holds bots, products and market
        // commentary too. Wikidata says which of them is a company.
        let claims = wiki.claims(&title).await;
        if !is_company(claims.as_ref()) {
            quiet.push(format!("{clean} (not a company)"));
            continue;
        }
        if is_listed(claims.as_ref()) {
            quiet.push(format!("{clean} (public)"));
            continue;
        }
        let rs = rounds_in(&text);
        if rs.is_empty() {
            quiet.push(clean);
            continue;
        }
        let url = format!(
            "https://en.wikipedia.org/wiki/{}",
            quote_path(&title.replace(' ', "_"))
        );
        // The first paragraph, which is what the article says it is.
        let para = sp().para.split(text.trim()).next().unwrap_or("").to_string();
        let total: f64 = rs.iter().map(|(v, _)| v).sum();
        found.push(Unheard {
            name: clean,
            total: total.round() as i64,
            rounds: rs.len(),
            url,
            blurb: head(sp().space.replace_all(&para, " ").as_ref(), 600),
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    Ok(Sweep { offered: offered.len(), fresh, found, quiet, notes })
}

pub fn write_unheard(con: &rusqlite::Connection, found: &[Unheard]) -> Result<usize> {
    let mut added = 0;
    for f in found {
        let pid = provider_id(&f.name);
        let taken: bool = con
            .query_row("SELECT 1 FROM providers WHERE id=?1", [&pid], |_| Ok(()))
            .is_ok();
        if taken {
            continue;
        }
        con.execute(
            "INSERT INTO providers (id,name,url,kind,raised,rounds,raised_source,listed) \
             VALUES (?1,?2,?3,'vendor',?4,?5,?3,0)",
            rusqlite::params![pid, f.name, f.url, f.total, f.rounds as i64],
        )?;
        write_description(con, &pid, &f.blurb, &f.url)?;
        added += 1;
    }
    Ok(added)
}

// ---------------------------------------------------------------------------
// The people behind the companies
// ---------------------------------------------------------------------------

impl Wiki {
    /// The English label of a Wikidata item — a person's name.
    pub async fn label(&self, qid: &str) -> Option<String> {
        let e: serde_json::Value = self
            .http
            .get(format!(
                "https://www.wikidata.org/wiki/Special:EntityData/{qid}.json"
            ))
            .timeout(std::time::Duration::from_secs(25))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        e["entities"][qid]["labels"]["en"]["value"]
            .as_str()
            .map(str::to_string)
    }
}

fn qids_of(claims: &serde_json::Value, prop: &str) -> Vec<String> {
    claims[prop]
        .as_array()
        .map(|a| {
            a.iter()
                // A former CEO or founder-who-left is still on the article as
                // a statement with an end-date qualifier (P582) or a
                // deprecated rank. Both must be dropped, or "who runs it"
                // reads "FormerCEO, CurrentCEO". Keep only current statements:
                // no P582 end date, and rank not deprecated.
                .filter(|c| {
                    c["rank"].as_str() != Some("deprecated")
                        && c["qualifiers"]["P582"].is_null()
                })
                .filter_map(|c| {
                    c["mainsnak"]["datavalue"]["value"]["id"].as_str().map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub struct Named {
    pub provider: String,
    pub field: &'static str,
    pub names: Vec<String>,
    pub source: String,
}

/// Who founded each company the catalogue holds, and who runs it — the two
/// people questions a reader asks first. Read off Wikidata (P112 founder,
/// P169 chief executive) for every company whose article the round reader
/// already found; nothing is guessed from prose.
pub async fn read_people(con: &rusqlite::Connection) -> Result<(usize, Vec<Named>)> {
    let mut q = con.prepare(
        "SELECT id, raised_source FROM providers \
          WHERE raised_source LIKE '%wikipedia.org/wiki/%'",
    )?;
    let todo: Vec<(String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    let wiki = Wiki::new()?;
    let mut out = Vec::new();
    for (pid, src) in &todo {
        let title = unquote(src.rsplit('/').next().unwrap_or("")).replace('_', " ");
        let Some(claims) = wiki.claims(&title).await else { continue };
        for (prop, field) in [("P112", "founded_by"), ("P169", "led_by")] {
            let mut names = Vec::new();
            for qid in qids_of(&claims, prop).iter().take(6) {
                if let Some(n) = wiki.label(qid).await {
                    names.push(n);
                }
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            if !names.is_empty() {
                out.push(Named {
                    provider: pid.clone(),
                    field,
                    names,
                    source: src.clone(),
                });
            }
        }
    }
    Ok((todo.len(), out))
}

pub fn write_people(con: &rusqlite::Connection, found: &[Named], today: &str) -> Result<usize> {
    let mut wrote = 0;
    for n in found {
        con.execute(
            "DELETE FROM docs WHERE subject=?1 AND kind='fact' AND field=?2",
            rusqlite::params![n.provider, n.field],
        )?;
        con.execute(
            "INSERT INTO docs (subject,kind,field,text,source_url,taken_at) \
             VALUES (?1,'fact',?2,?3,?4,?5)",
            rusqlite::params![n.provider, n.field, n.names.join(", "), n.source, today],
        )?;
        wrote += 1;
    }
    Ok(wrote)
}

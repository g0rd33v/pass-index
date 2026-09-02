//! What a thing is, said in our own words and only from what can be checked.
//!
//! The catalogue holds 2,351 descriptions written by the companies that sell
//! these things, and they cannot be the voice of a reference work. They
//! disagree with each other — one of Claude Opus 5's describes the Fast
//! variant instead — and they are written to sell. So nothing here is
//! borrowed. Every sentence is assembled from a figure the catalogue can show
//! a source for, and a clause whose fact is missing does not appear at all:
//! there is no "unknown", because a reader who sees a gap learns more from
//! silence than from a word that means nothing.
//!
//! Three parts, as agreed:
//!
//!   the line       one sentence, copyable, unique to the thing
//!   the paragraph  what it does, what it costs, where it stands
//!   the figures    every current number, copyable as plain text
//!
//! The last is the one an agent should take. It is also why the figures are a
//! list of pairs rather than prose: a machine reading this should not have to
//! parse English to find the price.

use serde_json::Value;

use crate::{grouped, money, ordinal, unit_phrase};

pub struct About {
    pub line: String,
    pub paragraph: String,
    /// (icon key, label, value) — the copyable block, in reading order.
    pub figures: Vec<(&'static str, &'static str, String)>,
}

/// A list as English writes it: "a, b and c", never "a, b, c".
fn and(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 { one.to_string() } else { format!("{n} {many}") }
}

/// Small numbers are written out when they open a sentence. "12 companies
/// "a" or "an", and the phrase after it.
///
/// The line is composed from the modality the source stated, so it is not
/// known at writing time whether it begins with a vowel: the page said "a
/// audio model" and "a images tool" for as long as the article was a letter
/// in a format string.
fn article(phrase: &str) -> String {
    let first = phrase.chars().next().unwrap_or('x').to_ascii_lowercase();
    let an = matches!(first, 'a' | 'e' | 'i' | 'o' | 'u');
    format!("{} {phrase}", if an { "an" } else { "a" })
}

/// sell it" is a table's voice; a reference work says twelve.
fn spelled(n: i64) -> String {
    const WORDS: [&str; 21] = [
        "No", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten",
        "Eleven", "Twelve", "Thirteen", "Fourteen", "Fifteen", "Sixteen", "Seventeen",
        "Eighteen", "Nineteen", "Twenty",
    ];
    if (0..=20).contains(&n) {
        WORDS[n as usize].to_string()
    } else {
        crate::grouped(n)
    }
}

/// A date as a sentence says it. "2026-08-14" is a field; "August 2026" is
/// what a person reads. The day is dropped on purpose — for a model it is
/// noise, and half the sources only ever knew the month. The preposition
/// belongs to the sentence, not to the date: baked in here it produced
/// "trained on material up to in January".
fn spoken_month(d: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June",
                                "July", "August", "September", "October", "November", "December"];
    let mut it = d.split('-');
    let (Some(y), Some(m)) = (it.next(), it.next()) else { return d.to_string() };
    match m.parse::<usize>() {
        Ok(n) if (1..=12).contains(&n) => format!("{} {}", MONTHS[n - 1], y),
        _ => y.to_string(),
    }
}

/// "text + image + file" reads as a machine's field; a sentence wants words.
fn kinds(s: &str) -> String {
    let parts: Vec<String> = s
        .split('+')
        .map(|p| match p.trim() {
            "text" => "text".into(),
            "code" => "code".into(),
            "image" => "images".into(),
            "audio" => "audio".into(),
            "video" => "video".into(),
            "file" => "files".into(),
            "embedding" => "vectors".into(),
            other => other.to_string(),
        })
        .collect();
    and(&parts)
}

fn licence_says(l: &str) -> Option<String> {
    Some(match l {
        "proprietary" => "Its weights are not published, so it is bought through an API and \
                          only from someone licensed to sell it.".into(),
        "apache-2.0" | "mit" | "bsd-3-clause" | "cc-by-4.0" | "openmdw-1.1" => format!(
            "Its weights are published under {}, so you may run it on your own machine, \
             or buy it from whoever serves it cheapest.",
            l.to_uppercase().replace("CC-BY-4.0", "CC BY 4.0")
        ),
        l if l.starts_with("cc-by-nc") => "Its weights are published, but the licence forbids \
                                           selling what it produces.".into(),
        l => format!(
            "Its weights are published under the {l} licence, which attaches conditions \
             the plain open licences do not."
        ),
    })
}

/// The cheapest and dearest standard-lane rate, and who charges them.
struct Spread {
    cheap: (String, i64, Option<i64>),
    dear: (String, i64, Option<i64>),
    dimension: String,
    sellers: usize,
    lanes: Vec<String>,
}

fn spread(v: &Value) -> Option<Spread> {
    let offerings = v["offerings"].as_array()?;
    let mut standard: Vec<(String, i64, Option<i64>, String)> = Vec::new();
    let mut lanes: Vec<String> = Vec::new();
    let mut sellers = std::collections::BTreeSet::new();
    for o in offerings {
        let who = o["provider"].as_str().unwrap_or("").to_string();
        sellers.insert(who.clone());
        let variant = o["variant"].as_str().unwrap_or("");
        if !variant.is_empty() && !lanes.contains(&variant.to_string()) {
            lanes.push(variant.to_string());
        }
        if !variant.is_empty() {
            continue;
        }
        let comps = o["components"].as_array().cloned().unwrap_or_default();
        let pick = |dim: &str| {
            comps
                .iter()
                .find(|c| c["dimension"].as_str() == Some(dim))
                .and_then(|c| c["micros_per_unit"].as_i64())
        };
        // Every rate this seller quotes, kept with its unit. Which unit the
        // card compares in is decided once, below, for the whole thing.
        for dim in ["mtok_in", "month", "image", "second", "minute", "call", "character",
                    "page", "result", "second_in", "second_out", "image_in"] {
            if let Some(p) = pick(dim) {
                let out = if dim == "mtok_in" { pick("mtok_out") } else { None };
                standard.push((who.clone(), p, out, dim.to_string()));
            }
        }
    }
    if standard.is_empty() {
        return None;
    }
    // One unit for the whole card, and it is the one most of its sellers use.
    //
    // Taking the first unit each seller happened to quote compared a price
    // per call with a price per result and called the smaller one cheapest:
    // Web search answered "how much is the cheapest" three different ways on
    // one page — $0.001 in the headline, $0.0003 in the table, $0.0002 in the
    // paragraph — because each was measuring something else.
    let mut per_dim: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, _, _, d) in &standard {
        *per_dim.entry(d.as_str()).or_default() += 1;
    }
    const LEADS: [&str; 12] = ["mtok_in", "month", "image", "second", "minute", "call",
                               "character", "page", "result", "second_in", "second_out",
                               "image_in"];
    let chosen = per_dim
        .iter()
        .max_by_key(|(d, n)| {
            (**n, std::cmp::Reverse(LEADS.iter().position(|x| x == *d).unwrap_or(99)))
        })
        .map(|(d, _)| d.to_string())?;
    let mut standard: Vec<(String, i64, Option<i64>, String)> =
        standard.into_iter().filter(|(_, _, _, d)| *d == chosen).collect();
    if standard.is_empty() {
        return None;
    }
    standard.sort_by_key(|(_, p, _, _)| *p);
    let cheap = standard.first()?.clone();
    let dear = standard.last()?.clone();
    Some(Spread {
        dimension: cheap.3.clone(),
        cheap: (cheap.0, cheap.1, cheap.2),
        dear: (dear.0, dear.1, dear.2),
        sellers: sellers.len(),
        lanes,
    })
}

/// A lane is a way of buying the same thing more cheaply or more quickly, but
/// the strings sellers use are mostly routing — "bedrock eu-west-3",
/// "google-vertex/global". Naming twenty-two of them tells a reader nothing
/// and costs them a paragraph; naming the kinds tells them what choices exist.
fn lane_families(lanes: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |w: &str| {
        if !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    };
    for l in lanes {
        let l = l.to_lowercase();
        if l.contains("batch") {
            push("batch");
        } else if l.contains("flex") {
            push("flex");
        } else if l.contains("fast") || l.contains("priority") {
            push("faster");
        } else if l.contains('/') || l.contains("region") || l.contains(" eu") ||
                  l.contains(" us") || l.contains("global") {
            push("regional");
        } else {
            push("separately routed");
        }
    }
    out
}

fn unit_of(dim: &str) -> &'static str {
    unit_phrase(dim)
}

fn rate(p: i64, out: Option<i64>, dim: &str) -> String {
    match out {
        Some(o) => format!("{} in and {} out {}", money(p), money(o), unit_of(dim)),
        None => format!("{} {}", money(p), unit_of(dim)),
    }
}

/// The best placement a thing holds, judged by the share of the field it beat
/// rather than by the rank: third of four hundred is a larger claim than
/// second of five, and ranking on the number alone reverses them.
fn best_standing(v: &Value) -> Option<(i64, i64, String)> {
    let mut best: Option<(f64, i64, i64, String)> = None;
    for b in v["benchmarks"].as_array()? {
        let (r, o) = (b["rank"].as_i64()?, b["out_of"].as_i64()?);
        if o <= 1 {
            continue;
        }
        let share = (o - r) as f64 / (o - 1) as f64;
        let name = b["suite_name"].as_str().unwrap_or("").to_string();
        // First of four hundred and first of five are both a perfect share;
        // the larger field is the harder thing to have done.
        let better = match &best {
            None => true,
            Some((s, _, bo, _)) => share > *s + 1e-9 || ((share - *s).abs() < 1e-9 && o > *bo),
        };
        if better {
            best = Some((share, r, o, name));
        }
    }
    best.map(|(_, r, o, n)| (r, o, n))
}

pub fn entity(v: &Value) -> About {
    let name = v["name"].as_str().unwrap_or("").to_string();
    let register = v["register"].as_str().unwrap_or("model");
    let maker = v["maker_name"].as_str().unwrap_or("").to_string();
    let takes = v["input_kind"].as_str().unwrap_or("");
    let gives = v["output_kind"].as_str().unwrap_or("");
    let attrs = &v["attrs"];
    let sp = spread(v);
    let stand = best_standing(v);

    // ---- the line ----------------------------------------------------------
    let mut line = name.clone();
    line.push_str(" — ");
    line.push_str(&article(&if takes.is_empty() {
        register.to_string()
    } else {
        format!("{} {register}", kinds(takes))
    }));
    if !maker.is_empty() {
        line.push_str(&format!(" from {maker}"));
    }
    // A thing sold only by the month has no rate, and saying nothing about
    // what it costs is the one thing a catalogue of prices must not do.
    let plans = v["plans"].as_array().cloned().unwrap_or_default();
    let cheapest_plan = plans.iter().filter_map(|p| p["month"].as_i64()).min();
    if let Some(s) = &sp {
        line.push_str(&format!(
            ", sold by {} from {}",
            plural(s.sellers as i64, "one company", "companies"),
            rate(s.cheap.1, s.cheap.2, &s.dimension)
        ));
    } else if let Some(m) = cheapest_plan {
        line.push_str(&format!(
            ", sold by the month on {} from {} a month",
            plural(plans.len() as i64, "one plan", "plans"),
            money(m)
        ));
    }
    if let Some((r, o, board)) = &stand {
        line.push_str(&format!(", placed {} of {} on {board}", ordinal(*r), grouped(*o)));
    }
    line.push('.');

    // ---- the paragraph -----------------------------------------------------
    let mut p: Vec<String> = Vec::new();
    if !takes.is_empty() && !gives.is_empty() {
        let context = attrs["context"]
            .as_i64()
            .map(|c| format!(", with a context window of {} tokens", grouped(c)))
            .unwrap_or_default();
        p.push(format!("It takes {} and returns {}{context}.", kinds(takes), kinds(gives)));
    }
    // When it came out and what it was trained up to. A model's age is the
    // fact a reader supplies from memory when the page does not, and memory
    // is wrong about a market that ships weekly.
    if let Some(d) = attrs["released"].as_str() {
        let cut = attrs["knowledge"]
            .as_str()
            .map(|k| format!(", trained on material up to {}", spoken_month(k)))
            .unwrap_or_default();
        p.push(format!("It was published in {}{cut}.", spoken_month(d)));
    }
    // Two capabilities a buyer checks before anything else, and neither is
    // visible from a price. Silence means nobody said, not that it cannot.
    let can: Vec<String> = [("reasoning", "reason step by step"), ("tool_call", "call a tool")]
        .iter()
        .filter(|(k, _)| attrs[*k].as_bool().unwrap_or(false))
        .map(|(_, w)| (*w).to_string())
        .collect();
    if !can.is_empty() {
        p.push(format!("Its sellers say it can {}.", and(&can)));
    }
    if let Some(tasks) = attrs["tasks"].as_array() {
        let words: Vec<String> = tasks.iter().filter_map(|t| t.as_str()).map(str::to_string).collect();
        if !words.is_empty() {
            p.push(format!("The catalogue files it under {}.", and(&words)));
        }
    }
    if let Some(l) = attrs["license"].as_str().and_then(licence_says) {
        p.push(l);
    }
    if let Some(s) = &sp {
        if s.sellers <= 1 {
            p.push(format!(
                "Only {} sells it, at {}.",
                s.cheap.0,
                rate(s.cheap.1, s.cheap.2, &s.dimension)
            ));
        } else {
            let mut sentence = format!(
                "{} sell it. The cheapest is {} at {}",
                format!("{} companies", spelled(s.sellers as i64)),
                rate(s.cheap.1, s.cheap.2, &s.dimension),
                s.cheap.0
            );
            sentence.push('.');
            p.push(sentence);
        }
        let families = lane_families(&s.lanes);
        if !families.is_empty() {
            p.push(format!(
                "Beside the standard rate there {} {} lane{}.",
                if families.len() == 1 { "is a" } else { "are" },
                and(&families),
                if families.len() == 1 { "" } else { "s" }
            ));
        }
    }
    if sp.is_none() && !plans.is_empty() {
        let names: Vec<String> = plans
            .iter()
            .filter_map(|p| {
                Some(format!(
                    "{} at {} a month",
                    p["name"].as_str()?,
                    money(p["month"].as_i64()?)
                ))
            })
            .collect();
        p.push(format!(
            "It is not sold by the unit. {}: {}. What each plan allows is on its own page, \
             because a monthly price without its cap is half a price.",
            if names.len() == 1 { "There is one plan" } else { "The plans are" },
            and(&names)
        ));
    }

    let boards = v["benchmarks"].as_array().map(|b| {
        b.iter().filter_map(|x| x["suite"].as_str()).collect::<std::collections::BTreeSet<_>>().len()
    }).unwrap_or(0);
    if let Some((r, o, board)) = &stand {
        p.push(if boards > 1 {
            format!(
                "It has been measured on {boards} boards, and stands best at {} of {} on {board}.",
                ordinal(*r), grouped(*o)
            )
        } else {
            format!("It stands {} of {} on {board}.", ordinal(*r), grouped(*o))
        });
    }

    // ---- the figures -------------------------------------------------------
    let mut f: Vec<(&'static str, &'static str, String)> = Vec::new();
    if !maker.is_empty() {
        f.push(("maker", "Maker", maker.clone()));
    }
    f.push(("kind", "Register", register.to_string()));
    if !takes.is_empty() {
        f.push(("in", "Takes", takes.to_string()));
    }
    if !gives.is_empty() {
        f.push(("out", "Returns", gives.to_string()));
    }
    if let Some(c) = attrs["context"].as_i64() {
        f.push(("context", "Context", format!("{} tokens", grouped(c))));
    }
    if let Some(m) = attrs["max_output"].as_i64().filter(|m| *m > 0) {
        f.push(("context", "Longest answer", format!("{} tokens", grouped(m))));
    }
    if let Some(d) = attrs["released"].as_str() {
        f.push(("released", "Published", d.to_string()));
    }
    if let Some(k) = attrs["knowledge"].as_str() {
        f.push(("released", "Knowledge to", k.to_string()));
    }
    if let Some(pm) = attrs["params"].as_f64() {
        f.push(("size", "Parameters", format!("{:.0} billion", pm / 1e9)));
    }
    if let Some(l) = attrs["license"].as_str() {
        f.push(("licence", "Licence", l.to_string()));
    }
    if let Some(s) = &sp {
        f.push(("sellers", "Sellers", grouped(s.sellers as i64)));
        f.push(("price", "Price",
                format!("{} — {}", rate(s.cheap.1, s.cheap.2, &s.dimension), s.cheap.0)));
    }
    if boards > 0 {
        f.push(("board", "Boards", grouped(boards as i64)));
    }
    if let Some((r, o, board)) = &stand {
        f.push(("board", "Best place", format!("{} of {} — {board}", ordinal(*r), grouped(*o))));
    }

    About { line, paragraph: p.join(" "), figures: f }
}

/// How many of each kind, said as English: "320 models, six tools and two
/// agents" rather than a row of counts.
fn registers(list: &[Value]) -> Vec<String> {
    let mut by: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
    for x in list {
        *by.entry(x["register"].as_str().unwrap_or("thing")).or_insert(0) += 1;
    }
    // Models, then tools, then agents: the order the catalogue lists its
    // registers in everywhere else, rather than the order of the alphabet.
    let mut out = Vec::new();
    for r in ["model", "tool", "agent", "subscription", "thing"] {
        if let Some(n) = by.remove(r) {
            out.push(format!("{} {r}{}", small(n), if n == 1 { "" } else { "s" }));
        }
    }
    for (r, n) in by {
        out.push(format!("{} {r}{}", small(n), if n == 1 { "" } else { "s" }));
    }
    out
}

/// The same rule as `spelled`, mid-sentence: "one agent and 27 models".
fn small(n: i64) -> String {
    const W: [&str; 11] = ["no", "one", "two", "three", "four", "five", "six", "seven",
                           "eight", "nine", "ten"];
    if (0..=10).contains(&n) { W[n as usize].to_string() } else { grouped(n) }
}

/// Every product's address begins with whoever made it, so the makers a
/// seller stands between can be counted off its own list.
fn makers_behind(list: &[Value]) -> usize {
    list.iter()
        .filter_map(|x| x["href"].as_str())
        .filter_map(|h| h.strip_prefix("/index/"))
        .filter_map(|h| h.split('/').next())
        .filter(|m| !m.is_empty() && *m != "commons")
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

pub fn provider(v: &Value) -> About {
    let name = v["name"].as_str().unwrap_or("").to_string();
    let kind = v["provider_kind"].as_str().unwrap_or("vendor");
    let empty: Vec<Value> = Vec::new();
    let resells = v["resells"].as_array().unwrap_or(&empty);
    let makes = v["makes"].as_array().unwrap_or(&empty);
    let home = v["url"].as_str().unwrap_or("");
    let behind = makers_behind(resells);

    let what = match kind {
        "aggregator" => "an aggregator — it resells other companies' work",
        "cloud" => "a cloud — it rents models beside the rest of its infrastructure",
        // A fund sells nothing. What it has is a portfolio, and saying it has
        // nothing priced is answering a question nobody asked of it.
        "fund" => "an investor",
        _ => "a maker — it sells what it builds",
    };
    let leads_with_own = kind == "vendor" && makes.len() >= resells.len();
    let mut line = format!("{name} is {what}");
    if kind == "fund" {
        let n = v["portfolio"].as_array().map(|p| p.len()).unwrap_or(0);
        line.push_str(&match n {
            0 => " — no company here names it as a backer yet.".to_string(),
            1 => " — one company here names it as a backer.".to_string(),
            n => format!(" — {n} companies here name it as a backer."),
        });
        let figures = vec![
            ("home", "Home", home.to_string()),
            ("sellers", "Backed", grouped(n as i64)),
        ];
        return About { line, paragraph: String::new(), figures };
    }
    if leads_with_own && !makes.is_empty() {
        line.push_str(&format!(", {} in the catalogue", and(&registers(makes))));
    } else if !resells.is_empty() {
        line.push_str(&format!(", {} on offer", and(&registers(resells))));
        if behind > 1 {
            line.push_str(&format!(" from {behind} companies"));
        }
    } else if !makes.is_empty() {
        line.push_str(&format!(", {} in the catalogue", and(&registers(makes))));
    } else {
        line.push_str(", with nothing priced here yet");
    }
    line.push('.');

    let mut p: Vec<String> = Vec::new();
    if !makes.is_empty() && !resells.is_empty() && !leads_with_own {
        p.push(format!(
            "It makes {} of its own, and its price list runs to {} in all.",
            and(&registers(makes)),
            and(&registers(resells))
        ));
    } else if !makes.is_empty() {
        let one = makes.len() == 1;
        p.push(format!(
            "It makes {}, and the catalogue holds what {} and who else sells {}.",
            and(&registers(makes)),
            if one { "it costs" } else { "each costs" },
            if one { "it" } else { "them" }
        ));
        if !resells.is_empty() {
            p.push(format!("It also resells {}.", and(&registers(resells))));
        }
    } else if !resells.is_empty() {
        p.push(format!(
            "Its price list runs to {}, none of it its own work.",
            and(&registers(resells))
        ));
    }
    if behind > 1 {
        p.push(format!(
            "What it sells was built by {behind} different companies, so buying here is one \
             account instead of {behind}."
        ));
    }
    if resells.is_empty() && makes.is_empty() {
        p.push("Nothing it sells has been priced into the catalogue yet — the company is \
                here as a name the market uses, and the page will fill as its prices are \
                read.".into());
    }

    let mut f: Vec<(&'static str, &'static str, String)> = Vec::new();
    f.push(("kind", "Kind", kind.to_string()));
    if !makes.is_empty() {
        f.push(("maker", "Makes", grouped(makes.len() as i64)));
    }
    if !resells.is_empty() {
        f.push(("sellers", "On its price list", grouped(resells.len() as i64)));
    }
    if behind > 1 {
        f.push(("maker", "Companies behind it", grouped(behind as i64)));
    }
    if !home.is_empty() {
        f.push(("link", "Home", home.to_string()));
    }

    About { line, paragraph: p.join(" "), figures: f }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_fact_removes_its_clause_rather_than_saying_unknown() {
        let a = entity(&json!({
            "name": "Nameless", "register": "model",
            "input_kind": "text", "output_kind": "text",
            "attrs": {}, "offerings": [], "benchmarks": []
        }));
        assert!(!a.line.contains("unknown"), "{}", a.line);
        assert!(!a.paragraph.contains("unknown"), "{}", a.paragraph);
        assert!(!a.paragraph.contains("0 "), "{}", a.paragraph);
    }

    #[test]
    fn the_line_names_the_thing_the_maker_and_the_price() {
        let a = entity(&json!({
            "name": "Thing", "register": "model", "maker_name": "Somebody",
            "input_kind": "text + image", "output_kind": "text",
            "attrs": {"context": 200000},
            "offerings": [{"provider": "A", "variant": "", "components": [
                {"dimension": "mtok_in", "micros_per_unit": 1000000},
                {"dimension": "mtok_out", "micros_per_unit": 5000000}]}],
            "benchmarks": []
        }));
        assert!(a.line.starts_with("Thing — a text and images model from Somebody"), "{}", a.line);
        assert!(a.line.contains("$1"), "{}", a.line);
        assert!(a.paragraph.contains("200,000 tokens"), "{}", a.paragraph);
    }

    #[test]
    fn a_wide_spread_is_stated_as_a_multiple() {
        let a = entity(&json!({
            "name": "T", "register": "model", "input_kind": "text", "output_kind": "text",
            "attrs": {},
            "offerings": [
              {"provider": "Cheap", "variant": "", "components": [
                {"dimension": "mtok_in", "micros_per_unit": 1000000}]},
              {"provider": "Dear", "variant": "", "components": [
                {"dimension": "mtok_in", "micros_per_unit": 4000000}]}],
            "benchmarks": []
        }));
        assert!(a.paragraph.contains("companies sell it"), "{}", a.paragraph);
    }
}

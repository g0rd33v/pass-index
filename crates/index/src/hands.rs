//! The jobs that were run by hand, in the language everything else is in.
//!
//! Two of them: a licence Hugging Face knows and the catalogue does not, and
//! the catalogue written out whole for somebody to read offline.

use anyhow::Result;

// ---------------------------------------------------------------------------
// A licence Hugging Face knows and the catalogue does not
// ---------------------------------------------------------------------------

pub struct Licence {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub licence: String,
    pub sellers: i64,
}

/// Ask Hugging Face for a licence by model name, not by repository path.
///
/// The alias table holds seller strings — "deepseek/deepseek-v3.2" is what
/// OpenRouter calls it, not where the weights live. Searching by name finds
/// the repository those strings point at. A match is accepted only when the
/// repository name reduces to exactly the model name, because a near-match
/// here writes a wrong licence into the field a reader trusts most.
///
/// Served politely: Hugging Face answers 429 to a fan-out, so this walks.
pub async fn read_licences(con: &rusqlite::Connection) -> Result<(usize, Vec<Licence>)> {
    let mut q = con.prepare(
        "SELECT e.id, e.name, COALESCE(p.name,''), COUNT(DISTINCT o.provider_id) n \
           FROM entities e \
           LEFT JOIN providers p ON p.id = e.maker \
           JOIN offerings o ON o.entity_id = e.id WHERE e.register='model' \
          AND json_extract(e.attrs,'$.license') IS NULL GROUP BY e.id ORDER BY n DESC",
    )?;
    let todo: Vec<(String, String, String, i64)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    let http = reqwest::Client::builder()
        .user_agent("pass-index/1.0")
        .timeout(std::time::Duration::from_secs(25))
        .build()?;
    let walk = std::time::Duration::from_millis(350);

    let mut found = Vec::new();
    for (i, (eid, name, maker, sellers)) in todo.iter().enumerate() {
        let want = crate::resolve::norm(name);
        let hits = ask(&http, &format!(
            "https://huggingface.co/api/models?search={}&limit=20",
            crate::prose::quote_path(name)
        )).await;
        tokio::time::sleep(walk).await;
        for h in hits.as_ref().and_then(|v| v.as_array()).map(Vec::as_slice).unwrap_or(&[]) {
            let rid = h["id"].as_str().unwrap_or("");
            let leaf = rid.rsplit('/').next().unwrap_or("");
            if crate::resolve::norm(leaf) != want && crate::resolve::norm(&rid.replace('/', "")) != want {
                continue;
            }
            // The repository must belong to whoever made the model. A name
            // match alone accepted anybody's re-upload, and a re-upload
            // carries whatever licence its uploader typed — the field a
            // reader trusts most, taken from the person least entitled to
            // state it. No maker on record, no licence from here.
            let owner = rid.split('/').next().unwrap_or("");
            let ob = crate::resolve::norm(owner);
            let mb = crate::resolve::norm(maker);
            let owners_own = !mb.is_empty()
                && (ob == mb
                    || (mb.chars().count() >= 4 && ob.contains(&mb))
                    || crate::prose::same_company(owner, maker));
            if !owners_own {
                continue;
            }
            let card = ask(&http, &format!("https://huggingface.co/api/models/{rid}")).await;
            tokio::time::sleep(walk).await;
            let lic = card.as_ref().map(|d| &d["cardData"]["license"]).and_then(|l| {
                l.as_str().map(str::to_string)
                    .or_else(|| l.as_array().and_then(|a| a.first())
                        .and_then(|v| v.as_str()).map(str::to_string))
            });
            if let Some(lic) = lic {
                found.push(Licence {
                    id: eid.clone(),
                    name: name.clone(),
                    repo: rid.to_string(),
                    licence: lic.to_lowercase(),
                    sellers: *sellers,
                });
                break;
            }
        }
        if i % 50 == 0 {
            println!("  {i}/{}, {} found", todo.len(), found.len());
        }
    }
    Ok((todo.len(), found))
}

/// Hugging Face answers 429 to a fan-out; each refusal buys a longer wait.
async fn ask(http: &reqwest::Client, url: &str) -> Option<serde_json::Value> {
    for attempt in 0..4u64 {
        match http.get(url).send().await {
            Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                tokio::time::sleep(std::time::Duration::from_secs(4 * (attempt + 1))).await;
            }
            Ok(r) if r.status().is_success() => return r.json().await.ok(),
            _ => return None,
        }
    }
    None
}

pub fn write_licences(con: &rusqlite::Connection, found: &[Licence]) -> Result<()> {
    for f in found {
        con.execute(
            "UPDATE entities SET attrs=json_set(COALESCE(attrs,'{}'),'$.license',?1) \
             WHERE id=?2",
            rusqlite::params![f.licence, f.id],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The catalogue as documents, for a knowledge base to read
// ---------------------------------------------------------------------------
//
// A knowledge base retrieves passages, so the shape that matters is not the
// database's but the reader's: each document answers one kind of question and
// carries enough around each fact that a passage lifted out of it still makes
// sense on its own. A row saying "$4 → $20" is useless in isolation; "Claude
// Opus 5, made by Anthropic, costs $4 per million tokens in and $20 out at
// Anthropic, and fifteen companies sell it" survives being quoted.

const SITE: &str = "https://pass.io";

fn money(micros: Option<i64>) -> String {
    let Some(m) = micros else { return String::new() };
    let d = m as f64 / 1e6;
    if d >= 100.0 {
        return format!("${d:.0}");
    }
    if d >= 1.0 {
        return format!("${d:.2}");
    }
    let s = format!("{d:.8}");
    format!("${}", s.trim_end_matches('0').trim_end_matches('.'))
}

/// The unit is a phrase that follows the figure, not a noun the sentence can
/// lead with.
fn unit_phrase(dim: &str) -> String {
    match dim {
        "mtok_in" => "per million tokens in",
        "mtok_out" => "per million tokens out",
        "mtok_cache_read" => "per million tokens cached",
        "month" => "a month",
        "image" => "an image",
        "image_in" => "an image sent in",
        "second" => "a second",
        "second_in" => "a second of audio in",
        "second_out" => "a second of audio out",
        "minute" => "a minute",
        "call" => "a call",
        "character" => "a character",
        "page" => "a page",
        "result" => "a result",
        other => return format!("per {other}"),
    }
    .to_string()
}

fn commas(n: i64) -> String {
    crate::prose::with_commas(n)
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { one.to_string() } else { many.to_string() }
}

fn things(con: &rusqlite::Connection, register: &str, title: &str, intro: &str) -> Result<String> {
    let mut out = vec![format!("# {title}"), String::new(), intro.to_string(), String::new()];
    let mut q = con.prepare(
        "SELECT e.id, e.name, COALESCE(p.name,''), e.input_kind, e.output_kind, \
                COALESCE(e.attrs,'{}') \
           FROM entities e LEFT JOIN providers p ON p.id = e.maker \
          WHERE e.register = ?1 ORDER BY e.name COLLATE NOCASE",
    )?;
    let rows: Vec<(String, String, String, Option<String>, Option<String>, String)> = q
        .query_map([register], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    for (eid, name, maker, ink, outk, attrs) in rows {
        let a: serde_json::Value =
            serde_json::from_str(&attrs).unwrap_or(serde_json::json!({}));
        let mut q = con.prepare(
            "SELECT p.name, pr.dimension, MIN(pr.micros_per_unit) \
               FROM offerings o JOIN providers p ON p.id = o.provider_id \
               JOIN current_prices pr ON pr.offering_id = o.id \
              WHERE o.entity_id = ?1 AND COALESCE(o.variant,'') = '' \
                AND o.status = 'live' \
              GROUP BY p.name, pr.dimension ORDER BY 3",
        )?;
        let sellers: Vec<(String, String, i64)> = q
            .query_map([&eid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(q);
        let stand: Option<(i64, i64, String)> = con
            .query_row(
                "SELECT b.rank, b.out_of, COALESCE(s.name, b.suite) \
                   FROM benchmarks b LEFT JOIN suites s ON s.id = b.suite \
                  WHERE b.entity_id = ?1 AND b.rank IS NOT NULL AND b.out_of > 1 \
                  ORDER BY CAST(b.rank AS REAL)/b.out_of LIMIT 1",
                [&eid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();

        let mut line: Vec<String> = Vec::new();
        let who = if maker.is_empty() { String::new() } else { format!(" made by {maker},") };
        line.push(format!(
            "{name} is a {register}{who} which takes {} and returns {}.",
            ink.as_deref().filter(|s| !s.is_empty()).unwrap_or("text"),
            outk.as_deref().filter(|s| !s.is_empty()).unwrap_or("text"),
        ));
        if let Some(c) = a["context"].as_i64() {
            line.push(format!("Its context window is {} tokens.", commas(c)));
        }
        if let Some(r) = a["released"].as_str() {
            line.push(format!("It was published on {r}."));
        }
        if let Some(l) = a["license"].as_str() {
            line.push(format!("Its licence is {l}."));
        } else if a["open_weights"].as_bool() == Some(true) {
            line.push("Its weights are published.".into());
        }
        if let Some(t) = a["tasks"].as_array() {
            let ts: Vec<&str> = t.iter().filter_map(|v| v.as_str()).collect();
            if !ts.is_empty() {
                line.push(format!("The catalogue files it under {}.", ts.join(", ")));
            }
        }
        if let Some(l) = a["limits"].as_str() {
            line.push(format!("What the plan allows: {l}."));
        }
        if sellers.is_empty() {
            line.push("Nobody in the catalogue publishes a price for it.".into());
        } else {
            let mut names: Vec<&str> = sellers.iter().map(|s| s.0.as_str()).collect();
            names.sort();
            names.dedup();
            line.push(format!(
                "{} {} {} it: {}.",
                names.len(),
                plural(names.len(), "company", "companies"),
                plural(names.len(), "sells", "sell"),
                names.join(", ")
            ));
            let mut best: Vec<(String, String, i64)> = Vec::new();
            for (pname, dim, m) in &sellers {
                match best.iter_mut().find(|(d, _, _)| d == dim) {
                    Some(b) if *m < b.2 => { b.1 = pname.clone(); b.2 = *m }
                    Some(_) => {}
                    None => best.push((dim.clone(), pname.clone(), *m)),
                }
            }
            best.sort_by(|a, b| a.0.cmp(&b.0));
            for (dim, pname, m) in best {
                line.push(format!(
                    "The cheapest is {} {}, at {pname}.",
                    money(Some(m)),
                    unit_phrase(&dim)
                ));
            }
        }
        if let Some((rank, of, suite)) = stand {
            line.push(format!("Its best placing is {rank} of {of} on {suite}."));
        }
        line.push(format!("Page: {SITE}/index — search for {name}."));
        out.push(format!("## {name}\n\n{}", line.join(" ")));
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

fn companies(con: &rusqlite::Connection) -> Result<String> {
    let mut out = vec![
        "# Companies in the AI market".to_string(),
        String::new(),
        "Every company the catalogue holds: what it is, what it sells or builds, \
         whether it has taken venture money, and who backed it."
            .to_string(),
        String::new(),
    ];
    let mut q = con.prepare(
        "SELECT id, name, COALESCE(url,''), COALESCE(kind,'vendor'), raised, rounds, \
                COALESCE(backing,'') FROM providers ORDER BY name COLLATE NOCASE",
    )?;
    let rows: Vec<(String, String, String, String, Option<i64>, Option<i64>, String)> = q
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    for (pid, name, url, kind, raised, rounds, backing) in rows {
        let makes: i64 = con.query_row(
            "SELECT COUNT(*) FROM entities WHERE maker=?1", [&pid], |r| r.get(0))?;
        let sells: i64 = con.query_row(
            "SELECT COUNT(DISTINCT entity_id) FROM offerings WHERE provider_id=?1",
            [&pid], |r| r.get(0))?;
        let desc: Option<String> = con.query_row(
            "SELECT text FROM docs WHERE subject=?1 AND kind='description' LIMIT 1",
            [&pid], |r| r.get(0)).ok();
        let names_of = |sql: &str| -> Result<Vec<String>> {
            let mut q = con.prepare(sql)?;
            let got: Vec<String> = q
                .query_map([&pid], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(got)
        };
        let backers = names_of(
            "SELECT f.name FROM investments i JOIN providers f ON f.id=i.fund_id \
              WHERE i.company_id=?1 ORDER BY f.name")?;
        let portfolio = names_of(
            "SELECT c.name FROM investments i JOIN providers c ON c.id=i.company_id \
              WHERE i.fund_id=?1 ORDER BY c.name")?;

        let mut s: Vec<String> = Vec::new();
        s.push(if url.is_empty() {
            format!("{name} is filed as a {kind}.")
        } else {
            format!("{name} is filed as a {kind} and its site is {url}.")
        });
        if let Some(d) = desc {
            s.push(d);
        }
        if makes > 0 {
            s.push(format!("It makes {makes} thing{} in the catalogue.",
                           if makes == 1 { "" } else { "s" }));
        }
        if sells > 0 {
            s.push(format!("It sells {sells} thing{}.", if sells == 1 { "" } else { "s" }));
        }
        match raised.filter(|r| *r != 0) {
            // `raised` is dollars, not micro-dollars: it comes from a sentence
            // in an article, not from a rate card. Passing it through money(),
            // which divides by a million, reported OpenAI's $342 billion as
            // $342 million — a thousandfold error, stated as a fact, in the
            // document an assistant would quote.
            Some(r) => {
                let n = rounds.unwrap_or(0);
                s.push(format!(
                    "It has raised at least ${} across {n} round{} that we could read.",
                    commas(r), if n == 1 { "" } else { "s" }));
            }
            None if !backing.is_empty() => {
                s.push(format!("It is venture-backed: {backing}."));
            }
            None => {}
        }
        if !backers.is_empty() {
            s.push(format!("Its investors are {}.", backers.join(", ")));
        }
        if !portfolio.is_empty() {
            s.push(format!("As an investor it has backed {} companies: {}.",
                           portfolio.len(), portfolio.join(", ")));
        }
        out.push(format!("## {name}\n\n{}", s.join(" ")));
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

fn boards(con: &rusqlite::Connection) -> Result<String> {
    let mut out = vec![
        "# Leaderboards the catalogue reads".to_string(),
        String::new(),
        "Every board, who runs it, what it measures, and how the models we hold \
         placed on it."
            .to_string(),
        String::new(),
    ];
    let mut q = con.prepare(
        "SELECT id, name, COALESCE(measurer,''), COALESCE(metric,''), COALESCE(url,'') \
           FROM suites ORDER BY name COLLATE NOCASE",
    )?;
    let suites: Vec<(String, String, String, String, String)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);

    for (sid, name, measurer, metric, url) in suites {
        let mut q = con.prepare(
            "SELECT e.name, b.rank, b.out_of, b.value FROM benchmarks b \
               JOIN entities e ON e.id = b.entity_id \
              WHERE b.suite = ?1 AND b.rank IS NOT NULL ORDER BY b.rank LIMIT 40",
        )?;
        let rows: Vec<(String, i64, Option<i64>, f64)> = q
            .query_map([&sid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(q);
        if rows.is_empty() {
            continue;
        }
        let field = rows.iter().map(|r| r.2.unwrap_or(0)).max().unwrap_or(0);
        let source = if url.is_empty() { String::new() } else { format!(" Source: {url}.") };
        let head = format!(
            "{name} is run by {} and scores models on {}. It has ranked {field} models; \
             the catalogue holds {} of them.{source}",
            if measurer.is_empty() { "its own authors" } else { &measurer },
            if metric.is_empty() { "its own metric" } else { &metric },
            rows.len(),
        );
        let standings = rows
            .iter()
            .map(|(n, rank, _, v)| format!("{rank}. {n} ({})", trim_float(*v)))
            .collect::<Vec<_>>()
            .join("; ");
        out.push(format!(
            "## {name}\n\n{head} The standings, best first: {standings}."
        ));
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

/// What `%g` prints: six significant digits, trailing zeros dropped, and an
/// exponent once the number leaves the range a reader can hold. Rust's own
/// `{}` prints every digit it has, which turned a score of 4.06143 into
/// 4.061429 — the same number, spelled differently from every other document
/// the catalogue has published.
fn trim_float(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{v}");
    }
    let exp = v.abs().log10().floor() as i32;
    if !(-5..6).contains(&exp) {
        let raw = format!("{v:.5e}");
        let (mantissa, e) = raw.split_once('e').unwrap_or((raw.as_str(), "0"));
        let mantissa = if mantissa.contains('.') {
            mantissa.trim_end_matches('0').trim_end_matches('.')
        } else {
            mantissa
        };
        let n: i32 = e.parse().unwrap_or(0);
        return format!("{mantissa}e{}{:02}", if n < 0 { '-' } else { '+' }, n.abs());
    }
    let places = (5 - exp).max(0) as usize;
    let s = format!("{v:.places$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn vocabulary(con: &rusqlite::Connection) -> Result<String> {
    let mut out = vec![
        "# The vocabulary of the AI market".to_string(),
        String::new(),
        "What the words mean, in the terms a buyer needs them.".to_string(),
        String::new(),
    ];
    let mut q = con.prepare(
        "SELECT term, kind, short, body, also FROM terms ORDER BY term COLLATE NOCASE",
    )?;
    let rows: Vec<(String, String, String, String, Option<String>)> = q
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    drop(q);
    for (term, kind, short, body, also) in rows {
        let syn: Vec<String> = serde_json::from_str(also.as_deref().unwrap_or("[]"))
            .unwrap_or_default();
        let mut s = format!("## {term}\n\n{short} ({kind}.) {body}");
        if !syn.is_empty() {
            s.push_str(&format!(" Also called: {}.", syn.join(", ")));
        }
        s.push_str(&format!(" Page: {SITE}/index/tech."));
        out.push(s);
        out.push(String::new());
    }
    Ok(out.join("\n"))
}

/// Ingest is synchronous: the server extracts, describes, chunks and embeds
/// inside the request, so a document large enough to outlast a client's
/// patience leaves a half-finished row behind. Split on the headings, which is
/// also where a reader would split it.
const KB_LIMIT: usize = 40 * 1024;

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn split_on_headings(text: &str) -> Vec<String> {
    let (head, rest) = match text.split_once("\n## ") {
        Some((h, r)) => (h.to_string(), r.to_string()),
        None => (text.to_string(), String::new()),
    };
    let lead = head.split("\n\n").next().unwrap_or("").to_string();
    let mut parts = Vec::new();
    let mut cur = head;
    if !rest.is_empty() {
        for block in rest.split("\n## ") {
            let block = format!("## {block}");
            // Counted in characters, as the limit was written: a document
            // full of "·" and "—" is shorter to a reader than to a byte
            // count, and cutting on bytes moved every boundary.
            if chars(&cur) + chars(&block) > KB_LIMIT && !cur.trim().is_empty() {
                parts.push(cur);
                cur = format!("{lead}\n\n{block}");
            } else {
                cur = format!("{cur}\n\n{block}");
            }
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

pub fn export_kb(con: &rusqlite::Connection, out_dir: &str) -> Result<usize> {
    std::fs::create_dir_all(out_dir)?;
    let docs: Vec<(&str, String)> = vec![
        ("pass-index-models.md", things(con, "model",
            "Models in the AI market, with prices",
            "Every model the Pass Index holds: who made it, what it takes and \
             returns, what it costs and at which company, and where it places.")?),
        ("pass-index-tools.md", things(con, "tool", "AI tools, with prices",
            "Tools sold in the AI market and what each costs.")?),
        ("pass-index-agents.md", things(con, "agent", "AI agents, with prices",
            "Agents sold in the AI market and what each costs.")?),
        ("pass-index-subscriptions.md", things(con, "subscription",
            "AI subscriptions and what they allow",
            "Plans bought by the month, their price and their allowance.")?),
        ("pass-index-companies.md", companies(con)?),
        ("pass-index-boards.md", boards(con)?),
        ("pass-index-vocabulary.md", vocabulary(con)?),
    ];
    let mut written = 0;
    for (name, text) in &docs {
        let stem = &name[..name.len() - 3];
        let parts = split_on_headings(text);
        for (i, part) in parts.iter().enumerate() {
            let fname = if parts.len() == 1 {
                format!("{stem}.md")
            } else {
                format!("{stem}-{:02}.md", i + 1)
            };
            std::fs::write(std::path::Path::new(out_dir).join(fname), part)?;
            written += 1;
        }
        println!("{:<34} {:>7.1} KB in {} file{}", name, chars(text) as f64 / 1024.0,
                 parts.len(), if parts.len() == 1 { "" } else { "s" });
    }
    println!("\n{written} files");
    Ok(written)
}

//! What the nightly run mends, in the language the rest of the catalogue is
//! written in.
//!
//!   repair <db> naming [--apply]
//!       one spelling per brand, decided by the company that owns it
//!   repair <db> normalise [--apply]
//!       corrections the feeds keep re-introducing
//!   repair <db> aliases [--apply]
//!       an alias filed on a row further from its own text than another's
//!   repair <db> fold [--apply]
//!       rows that are the same thing written two ways
//!   repair <db> sizes [--apply]
//!       how big a model is, where its own name says so
//!   repair <db> opaque [--apply]
//!       companies kept although nobody can price them, and why
//!   repair <db> tasks [--apply]
//!       what each thing is for, from evidence the catalogue holds
//!   repair <db> terms [--apply]
//!       the vocabulary, written whole from data/terms.json
//!   repair <db> freetiers [--apply]
//!       what somebody runs for you and charges nothing, with the cap
//!   repair <db> plans [--apply]
//!       plans bought by the month, and what each allows
//!   repair <db> weights [--apply]
//!       whether the weights are published, by a vote of the sellers
//!   repair <db> free [--apply]
//!       what one seller declares free, on the lane that says so
//!   repair <db> facts [--apply]
//!       what a thing is beyond what it costs, from the price feeds
//!   repair <db> newmodels [--apply]
//!       models the market sells that the catalogue does not hold
//!   repair <db> sellers [--apply]
//!       the two public price files, filtered to the sellers we chose
//!   repair <db> boards [--apply]
//!       where a thing places, on boards other people run
//!   repair <db> dvc [--apply]
//!       one fund's portfolio, read off its own job board
//!   repair <db> startups [--limit N] [--apply]
//!       how much a company raised, added up out of the rounds we can read
//!   repair <db> funds [--apply]
//!       who put the money in, named by the same round sentences
//!   repair <db> enrich --fund <name> [--apply]
//!       what one fund's portfolio says about itself, cheapest source first
//!   repair <db> yc [--apply]
//!       Y Combinator's own directory, where membership is the evidence
//!   repair <db> discover [--limit N] [--apply]
//!       companies in this market the catalogue has never heard of
//!   repair <db> licences [--apply]
//!       the licence Hugging Face states, for models that hold none
//!   repair <db> retire [--apply]
//!       offerings the seller has stopped listing, taken off the shelf
//!   repair <db> supply --from <dir> [--pen <path>] [--apply]
//!       an outside supplier's findings, through the standard door
//!   repair <db> people [--apply]
//!       who founded each company and who runs it, from Wikidata
//!   repair <db> descriptions [--apply]
//!       one description per thing, chosen by a rule rather than by row order
//!   repair <db> quarantine [--pen <path>] [--apply]
//!       hold what only an unvetted company sells, in a second database
//!
//! Without `--apply` it says what it would do and writes nothing.

use anyhow::Result;
use index::{feed, repair};

/// Some jobs read a feed, so the whole binary is async; the ones that only
/// touch the catalogue simply never await.
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(db), Some(job)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: repair <db> <naming|normalise|aliases|fold|sizes|opaque|tasks|quarantine|descriptions|terms|freetiers|plans|weights|free|facts|newmodels|sellers|boards|dvc|startups|funds|enrich|yc|discover|licences|retire|supply> [--apply]");
        std::process::exit(2);
    };
    let apply = args.iter().any(|a| a == "--apply");
    let con = common::db::open(db)?;
    index::prepare(&con)?;

    match job.as_str() {
        "naming" => {
            let r = repair::naming(&con)?;
            println!("brands one maker writes more than one way: {}", r.brands_split);
            println!("names to mend: {}", r.mends.len());
            for t in &r.ties {
                let sp: Vec<String> = t
                    .spellings
                    .iter()
                    .map(|(s, n)| format!("{s}: {n}"))
                    .collect();
                println!(
                    "   tied, left alone: {:<14} {:<18} {{{}}}",
                    t.brand,
                    t.maker.chars().take(18).collect::<String>(),
                    sp.join(", ")
                );
            }
            for m in r.mends.iter().take(15) {
                println!(
                    "   {:<32} -> {:<32} ({})",
                    m.was.chars().take(32).collect::<String>(),
                    m.now.chars().take(32).collect::<String>(),
                    m.why
                );
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            repair::apply_naming(&con, &r)?;
            println!(
                "\nmended {}, each keeping its old spelling as an alias",
                r.mends.len()
            );
        }
        "normalise" => {
            let started = std::time::Instant::now();
            let total = repair::normalise(&con, apply)?;
            if apply {
                println!("\ncorrected {total} rows");
            } else {
                println!("\ndry run; nothing written");
            }
            repair::record_run(
                &con,
                "normalise",
                total,
                total,
                0,
                started.elapsed().as_secs_f64(),
                "",
            )?;
        }
        "aliases" => {
            let rows = repair::misfiled(&con)?;
            println!(
                "aliases filed on a row further from their own text: {}",
                rows.len()
            );
            for m in &rows {
                println!(
                    "  {:<48} {:<34} -> {}",
                    m.alias.chars().take(46).collect::<String>(),
                    m.filed_on,
                    m.names
                );
            }
            if apply && !rows.is_empty() {
                repair::move_aliases(&con, &rows)?;
                println!(
                    "\nmoved {}; still misfiled: {}",
                    rows.len(),
                    repair::misfiled(&con)?.len()
                );
            }
        }
        "fold" => {
            repair::rehome(&con, apply)?;
            let funds = repair::fold_funds(&con, apply)?;
            println!(
                "{funds} funds written under two names{}",
                if apply { "" } else { " (dry run)" }
            );
            let twins = repair::fold_twin_offerings(&con, apply)?;
            println!(
                "{twins} lanes written twice at one price{}",
                if apply { "" } else { " (dry run)" }
            );
            println!();
            repair::fold_entities(&con, apply)?;
            println!(
                "\n{}",
                if apply { "folded" } else { "dry run; nothing written" }
            );
        }
        "sizes" => {
            repair::sizes(&con, apply)?;
        }
        "opaque" => {
            let started = std::time::Instant::now();
            let o = repair::opaque(&con)?;
            println!("companies kept on purpose : {}", o.known.len());
            if !o.absent.is_empty() {
                println!(
                    "  named here but not in the catalogue: {}",
                    o.absent.join(", ")
                );
            }
            println!("companies nobody has justified : {}", o.unjustified.len());
            for n in o.unjustified.iter().take(20) {
                println!("     {n}");
            }
            if apply {
                repair::apply_opaque(&con, &o)?;
                println!("\nwrote {} reasons", o.known.len());
            } else {
                println!("\ndry run; nothing written");
            }
            repair::record_run(
                &con,
                "opaque",
                repair::KEPT.len() as i64,
                o.known.len() as i64,
                0,
                started.elapsed().as_secs_f64(),
                "",
            )?;
        }
        "tasks" => {
            repair::tasks(&con, apply)?;
        }
        "quarantine" => {
            let pen = args
                .iter()
                .position(|a| a == "--pen")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| "/srv/pass-index/data/quarantine.db".into());
            let cell = common::db::open(&pen)?;
            cell.execute_batch(repair::PEN_SCHEMA)?;
            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            // Releasing is a write, and a dry run must not write: without the
            // gate a report-only pass was quietly deleting from the pen. And
            // an entity that "arrived" arrived because newmodels re-minted it
            // — the pen's whole point is to remember that refusal — so only
            // companies, which genuinely enter by other roads (a fund's
            // portfolio, Y Combinator), are ever released this way.
            if apply {
                let released = repair::release_arrived(&con, &cell)?;
                if released > 0 {
                    println!("companies that had arrived by another road, released: {released}");
                }
            }
            let (things, companies) = repair::hold_unvetted(&con, &cell, &today, apply)?;
            println!("to hold: {things} things, {companies} companies");
            if apply {
                let held: i64 =
                    cell.query_row("SELECT COUNT(*) FROM candidates", [], |r| r.get(0))?;
                println!("the pen now holds {held} candidates");
            } else {
                println!("dry run; the main catalogue is untouched");
            }
        }
        "descriptions" => {
            let n = repair::one_description(&con, apply)?;
            println!(
                "\n{}",
                if apply { format!("dropped {n}") } else { "dry run; nothing written".into() }
            );
        }
        "terms" => {
            repair::terms(&con, apply)?;
        }
        "freetiers" => {
            let started = std::time::Instant::now();
            let (lanes, plans) = repair::free_tiers(&con, apply)?;
            repair::record_run(
                &con,
                "free-tiers",
                (lanes + plans) as i64,
                (lanes + plans) as i64,
                0,
                started.elapsed().as_secs_f64(),
                "",
            )?;
        }
        "plans" => {
            let started = std::time::Instant::now();
            let n = repair::subscriptions(&con, apply)? as i64;
            repair::record_run(&con, "subscriptions", n, n, 0,
                started.elapsed().as_secs_f64(), "")?;
        }
        "weights" => {
            let doc: serde_json::Value = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(90))
                .build()?
                .get(feed::MODELS_DEV)
                .send()
                .await?
                .json()
                .await?;
            let mut r = index::resolve::Resolver::from_conn(&con)?;
            let w = feed::weigh(&con, &doc, &mut r)?;
            println!(
                "models.dev states the weights of {} things the catalogue holds",
                w.stated
            );
            println!("  already answered here      : {}", w.already);
            println!("  its sellers cannot agree   : {}", w.split);
            println!("  new: weights are published : {}", w.opened.len());
            println!("  new: weights are not       : {}", w.closed.len());
            if apply {
                let n = feed::write_weights(&con, &w)?;
                println!("wrote {n}");
            }
        }
        "free" => {
            let started = std::time::Instant::now();
            let doc: serde_json::Value = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(90))
                .build()?
                .get(feed::OPENROUTER_MODELS)
                .send()
                .await?
                .json()
                .await?;
            let offered = feed::given_away(&doc);
            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let mut r = index::resolve::Resolver::from_conn(&con)?;
            let (mut bound, mut unbound) = (Vec::new(), Vec::new());
            for o in &offered {
                match r.bind(&o.name).or_else(|| r.bind(&o.id)) {
                    Some(eid) => bound.push((eid, o)),
                    None => unbound.push(o.id.clone()),
                }
            }
            println!("{} offered without charge at OpenRouter", offered.len());
            println!("  bound to something the catalogue holds: {}", bound.len());
            println!("  no such thing here                    : {}", unbound.len());
            for i in &unbound {
                println!("     {i}");
            }
            println!();
            for (_, o) in &bound {
                println!(
                    "   {:<46} {}",
                    o.name.chars().take(46).collect::<String>(),
                    match &o.expires {
                        Some(e) => format!("free until {e}"),
                        None => "no end date given".into(),
                    }
                );
            }
            if apply {
                let n = feed::write_given(&con, &bound, &today)?;
                println!("\nwrote {n} free lanes");
            } else {
                println!("\ndry run; nothing written");
            }
            repair::record_run(&con, "openrouter-free", offered.len() as i64,
                bound.len() as i64, bound.len() as i64,
                started.elapsed().as_secs_f64(), "")?;
        }
        "facts" => {
            let started = std::time::Instant::now();
            let http = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(90))
                .build()?;
            let openrouter: serde_json::Value =
                http.get(feed::OPENROUTER_MODELS).send().await?.json().await?;
            let modelsdev: serde_json::Value =
                http.get(feed::MODELS_DEV).send().await?.json().await?;
            let mut r = index::resolve::Resolver::from_conn(&con)?;
            let said = feed::hear_facts(&openrouter, &modelsdev, &mut r);

            // What the catalogue already answers, and not by a feed.
            let mut have: std::collections::HashMap<String, serde_json::Value> =
                std::collections::HashMap::new();
            {
                let mut q = con.prepare("SELECT id, COALESCE(attrs,'{}') FROM entities")?;
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    let id: String = row.get(0)?;
                    let at: String = row.get(1)?;
                    have.insert(id, serde_json::from_str(&at).unwrap_or(serde_json::json!({})));
                }
            }

            // Grouped in the order first heard, so the report and the writes
            // do not move between runs.
            let mut order: Vec<(&'static str, String)> = Vec::new();
            let mut heard: std::collections::HashMap<(&'static str, String), Vec<serde_json::Value>> =
                std::collections::HashMap::new();
            for (fact, eid, v) in &said {
                let key = (*fact, eid.clone());
                heard.entry(key.clone()).or_insert_with(|| { order.push(key); Vec::new() }).push(v.clone());
            }

            let mut writes: Vec<(String, Vec<(&'static str, serde_json::Value)>)> = Vec::new();
            let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            let mut counts: Vec<(&'static str, usize, usize)> = Vec::new();
            let mut bump = |counts: &mut Vec<(&'static str, usize, usize)>, f: &'static str, ok: bool| {
                match counts.iter_mut().find(|(n, _, _)| *n == f) {
                    Some(c) => { if ok { c.1 += 1 } else { c.2 += 1 } }
                    None => counts.push((f, ok as usize, !ok as usize)),
                }
            };
            for (fact, eid) in &order {
                let answered = have.get(eid).map(|a| &a[*fact]).is_some_and(|v| {
                    !(v.is_null() || v.as_str() == Some("")
                      || v.as_array().is_some_and(|x| x.is_empty())
                      || v.as_object().is_some_and(|x| x.is_empty()))
                });
                if answered {
                    continue; // already answered, and not by a feed
                }
                let floor = if *fact == "released" {
                    heard.get(&("knowledge", eid.clone()))
                        .and_then(|k| feed::settle("knowledge", k, None))
                        .and_then(|v| v.as_str().map(str::to_string))
                } else {
                    None
                };
                let Some(v) = feed::settle(fact, &heard[&(*fact, eid.clone())], floor.as_deref())
                else {
                    bump(&mut counts, fact, false);
                    continue;
                };
                bump(&mut counts, fact, true);
                let i = *at.entry(eid.clone()).or_insert_with(|| {
                    writes.push((eid.clone(), Vec::new()));
                    writes.len() - 1
                });
                writes[i].1.push((fact, v));
            }

            let subjects: std::collections::HashSet<&String> =
                said.iter().map(|(_, e, _)| e).collect();
            println!("the price feeds also state facts about {} things we hold", subjects.len());
            counts.sort_by(|a, b| b.1.cmp(&a.1));
            let total: usize = counts.iter().map(|c| c.1).sum();
            for (f, ok, no) in &counts {
                println!("  {f:<11} to write: {ok:5}   no majority: {no}");
            }
            if apply {
                for (eid, facts) in &writes {
                    let parts: Vec<String> = facts.iter()
                        .map(|(f, v)| format!("{}: {}", serde_json::to_string(f).unwrap_or_default(),
                                              serde_json::to_string(v).unwrap_or_default()))
                        .collect();
                    con.execute(
                        "UPDATE entities SET attrs = json_patch(coalesce(attrs,'{}'), ?1) WHERE id = ?2",
                        (format!("{{{}}}", parts.join(", ")), eid),
                    )?;
                }
                println!("\nwrote {total} facts across {} things", writes.len());
            } else {
                println!("\ndry run; nothing written");
            }
            repair::record_run(&con, "model-facts", said.len() as i64, total as i64,
                total as i64, started.elapsed().as_secs_f64(), "")?;
        }
        "newmodels" => {
            let http = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(90))
                .build()?;
            let modelsdev_raw = http.get(feed::MODELS_DEV).send().await?.text().await?;
            let modelsdev: serde_json::Value = serde_json::from_str(&modelsdev_raw)?;
            let openrouter: serde_json::Value =
                http.get(feed::OPENROUTER_MODELS).send().await?.json().await?;
            let mut r = index::resolve::Resolver::from_conn(&con)?;

            let mut companies: Vec<String> = Vec::new();
            let mut makers: Vec<(String, String)> = Vec::new();
            {
                let mut q = con.prepare("SELECT id, name FROM providers")?;
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    let id: String = row.get(0)?;
                    let name: String = row.get(1)?;
                    companies.push(index::resolve::norm(&name));
                    makers.push((index::resolve::norm(&name), id));
                }
            }
            let found = feed::unplaced(&modelsdev, &modelsdev_raw, &openrouter, &mut r, &companies);

            let mut have: Vec<String> = Vec::new();
            {
                let mut q = con.prepare("SELECT id FROM entities")?;
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    have.push(row.get(0)?);
                }
            }
            // What the pen holds is a candidate somebody looked at and did
            // not let in. Minting it again would erase that judgement — and
            // did, nightly: newmodels re-minted exactly the rows quarantine
            // had swept the night before, the pen released them as "arrived",
            // and the two jobs chased each other around the same 300 rows.
            {
                let pen = args.iter().position(|a| a == "--pen")
                    .and_then(|i| args.get(i + 1).cloned())
                    .unwrap_or_else(|| "/data/quarantine.db".into());
                if std::path::Path::new(&pen).exists() {
                    let cell = common::db::open(&pen)?;
                    let mut q = cell.prepare("SELECT id FROM candidates")?;
                    let mut rows = q.query([])?;
                    let mut held = 0usize;
                    while let Some(row) = rows.next()? {
                        have.push(row.get(0)?);
                        held += 1;
                    }
                    println!("  the pen holds {held}; none of them will be re-minted");
                }
            }
            // A function, not a closure: the list grows as unknown sellers
            // are minted, and a closure holding it could not be alive while
            // that happens.
            fn look(makers: &[(String, String)], n: &str) -> Option<String> {
                let key = index::resolve::norm(n);
                makers.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
            }
            let lead = regex::Regex::new(&format!(
                r"(?i)^({})[\s._/-]+", index::resolve::VENDORS.join("|")))?;

            // Most sellers first, so the report reads as it did and the mint
            // order does not move.
            let mut keep: Vec<&feed::Unplaced> = found.iter().collect();
            keep.sort_by(|a, b| b.sellers.len().cmp(&a.sellers.len()));

            let mut rows: Vec<(String, &feed::Unplaced, Option<String>)> = Vec::new();
            let mut minted: Vec<String> = Vec::new();
            for v in &keep {
                if !v.quotes.iter().any(|(_, px)| !px.is_empty()) {
                    continue;
                }
                let eid = feed::mint_id(&v.name);
                if have.contains(&eid) || minted.contains(&eid) {
                    continue;
                }
                // Who made it: the feed's own path first, then the name's
                // first word.
                let maker = look(&makers, &v.maker)
                    .or_else(|| lead.captures(&v.raw).or_else(|| lead.captures(&v.name))
                        .and_then(|c| look(&makers, c.get(1).unwrap().as_str())))
                    .or_else(|| look(&makers, v.name.split_whitespace().next().unwrap_or("")));
                minted.push(eid.clone());
                rows.push((eid, v, maker));
            }

            let sold_by_one = found.iter().filter(|v| v.sellers.len() == 1).count();
            println!("priced names the catalogue cannot place : {}", found.len());
            println!("  unique models once serving is stripped: {}", found.len());
            println!("  sold by one company: {sold_by_one}, by two or more: {}",
                     found.len() - sold_by_one);
            println!("  to mint: {}  ({} of them with a maker we already hold)",
                     rows.len(), rows.iter().filter(|(_, _, m)| m.is_some()).count());
            println!();
            let quotes: usize = rows.iter().map(|(_, v, _)| v.quotes.iter().map(|(_, p)| p.len()).sum::<usize>()).sum();
            println!("  they arrive with {quotes} price rows from the feeds that named them");
            for (_, v, m) in rows.iter().take(20) {
                println!("   {:<42} {:<15} {} seller{}",
                    v.name.chars().take(42).collect::<String>(),
                    m.clone().unwrap_or_else(|| "—".into()),
                    v.sellers.len(), if v.sellers.len() == 1 { "" } else { "s" });
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }

            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let (mut wrote_off, mut wrote_px) = (0usize, 0usize);
            let non = regex::Regex::new(r"[^a-z0-9]+")?;
            for (eid, v, maker) in &rows {
                let (takes, gives, attrs) = feed::new_facts(&v.blob);
                let parts: Vec<String> = attrs.iter()
                    .map(|(k, val)| format!("{}: {}",
                        serde_json::to_string(k).unwrap_or_default(),
                        serde_json::to_string(val).unwrap_or_default()))
                    .collect();
                con.execute(
                    "INSERT OR IGNORE INTO entities (id,register,name,maker,input_kind,\
                     output_kind,attrs) VALUES (?1,'model',?2,?3,?4,?5,?6)",
                    rusqlite::params![eid, &v.name, maker, takes, gives,
                                      format!("{{{}}}", parts.join(", "))],
                )?;
                con.execute(
                    "INSERT OR IGNORE INTO aliases (source, alias, entity_id) \
                     VALUES ('newmodels', ?2, ?1)",
                    (eid, &v.name),
                )?;
                // The seller that named it also priced it.
                for (seller, px) in &v.quotes {
                    let pid = match look(&makers, seller) {
                        Some(p) => p,
                        None => {
                            let pid = format!("prov_{}",
                                non.replace_all(&seller.to_lowercase(), "-").trim_matches('-'));
                            con.execute(
                                "INSERT OR IGNORE INTO providers (id,name,url,kind) \
                                 VALUES (?1,?2,'','aggregator')",
                                (&pid, seller),
                            )?;
                            makers.push((index::resolve::norm(seller), pid.clone()));
                            pid
                        }
                    };
                    let existing: Option<i64> = con.query_row(
                        "SELECT id FROM offerings WHERE entity_id=?1 AND provider_id=?2 \
                          AND COALESCE(variant,'')=''", (eid, &pid), |r| r.get(0)).ok();
                    let oid = match existing {
                        Some(o) => o,
                        None => {
                            con.execute(
                                "INSERT INTO offerings (entity_id,provider_id,way,variant,\
                                 status,first_seen,last_seen) \
                                 VALUES (?1,?2,'aggregator','','live',?3,?3)",
                                rusqlite::params![eid, &pid, today])?;
                            wrote_off += 1;
                            con.last_insert_rowid()
                        }
                    };
                    let src = if seller == "openrouter" {
                        feed::OPENROUTER_MODELS
                    } else {
                        feed::MODELS_DEV
                    };
                    for (dim, micros) in px {
                        con.execute(
                            "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,\
                             source_url,taken_at) VALUES (?1,?2,?3,'declared',?4,?5)",
                            rusqlite::params![oid, dim, micros, src, today])?;
                        wrote_px += 1;
                    }
                }
            }
            println!("\nminted {} models, {wrote_off} ways to buy, {wrote_px} prices", rows.len());
        }
        "sellers" => {
            let started = std::time::Instant::now();
            let http = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(90))
                .build()?;
            let litellm_raw = http.get(feed::LITELLM).send().await?.text().await?;
            let litellm: serde_json::Value = serde_json::from_str(&litellm_raw)?;
            let modelsdev_raw = http.get(feed::MODELS_DEV).send().await?.text().await?;
            let modelsdev: serde_json::Value = serde_json::from_str(&modelsdev_raw)?;
            let offers = feed::price_files(&litellm, &litellm_raw, &modelsdev, &modelsdev_raw);
            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let mut r = index::resolve::Resolver::from_conn(&con)?;

            let mut have_prov: Vec<String> = Vec::new();
            {
                let mut q = con.prepare("SELECT id FROM providers")?;
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? { have_prov.push(row.get(0)?) }
            }
            let mut have_off: std::collections::HashMap<(String, String, String), i64> =
                std::collections::HashMap::new();
            {
                let mut q = con.prepare(
                    "SELECT entity_id, provider_id, COALESCE(variant,''), id FROM offerings")?;
                let mut rows = q.query([])?;
                while let Some(row) = rows.next()? {
                    have_off.insert((row.get(0)?, row.get(1)?, row.get(2)?), row.get(3)?);
                }
            }

            let mut matched: Vec<(&feed::Offer, String)> = Vec::new();
            let mut unmatched: Vec<(String, usize)> = Vec::new();
            let mut priceless = 0usize;
            for o in &offers {
                if o.px.is_empty() { priceless += 1; continue }
                match r.bind(&o.name) {
                    Some(eid) => matched.push((o, eid)),
                    None => match unmatched.iter_mut().find(|(n, _)| *n == o.name) {
                        Some(u) => u.1 += 1,
                        None => unmatched.push((o.name.clone(), 1)),
                    },
                }
            }

            let mut new_prov: Vec<(String, String, String, String)> = Vec::new();
            let mut seen_pair: Vec<(String, String, String)> = Vec::new();
            let (mut add_off, mut add_px) = (0usize, 0usize);
            for (o, eid) in &matched {
                if !have_prov.contains(&o.seller)
                    && !new_prov.iter().any(|(p, _, _, _)| *p == o.seller)
                {
                    new_prov.push((o.seller.clone(), o.seller_name.clone(),
                                   o.kind.clone(), o.url.clone()));
                }
                let key = (eid.clone(), o.seller.clone(), o.lane.clone());
                if !have_off.contains_key(&key) && !seen_pair.contains(&key) { add_off += 1 }
                seen_pair.push(key);
                add_px += o.px.len();
            }
            let missed: usize = unmatched.iter().map(|(_, c)| c).sum();
            println!("LiteLLM and models.dev, filtered to the sellers we chose");
            println!("  priced rows from a chosen seller : {}", matched.len() + missed);
            println!("  matched to a model we already hold: {}", matched.len());
            println!("  no model of that name here       : {missed}");
            println!("  listed with no price at all      : {priceless}");
            println!();
            let mut names: Vec<&String> = new_prov.iter().map(|(_, n, _, _)| n).collect();
            names.sort();
            println!("  companies to add   : {}  ({})", new_prov.len(),
                     names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
            println!("  ways to buy to add : {add_off}");
            println!("  price rows to write: {add_px}");
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            for (pid, pname, kind, url) in &new_prov {
                con.execute(
                    "INSERT OR IGNORE INTO providers (id,name,url,kind) VALUES (?1,?2,?3,?4)",
                    rusqlite::params![pid, pname, url, kind])?;
            }
            let (mut wrote_off, mut wrote_px) = (0usize, 0usize);
            for (o, eid) in &matched {
                let key = (eid.clone(), o.seller.clone(), o.lane.clone());
                let oid = match have_off.get(&key) {
                    Some(id) => {
                        con.execute("UPDATE offerings SET last_seen=?1 WHERE id=?2",
                                    (&today, id))?;
                        *id
                    }
                    None => {
                        con.execute(
                            "INSERT INTO offerings (entity_id,provider_id,way,variant,status,\
                             first_seen,last_seen) VALUES (?1,?2,?3,?4,'live',?5,?5)",
                            rusqlite::params![eid, &o.seller, feed::way_of(&o.kind),
                                              &o.lane, &today])?;
                        let id = con.last_insert_rowid();
                        have_off.insert(key, id);
                        wrote_off += 1;
                        id
                    }
                };
                for (dim, micros) in &o.px {
                    if *micros <= 0 { continue }
                    con.execute(
                        "DELETE FROM prices WHERE offering_id=?1 AND dimension=?2 \
                          AND source_url=?3",
                        rusqlite::params![oid, dim, o.src])?;
                    con.execute(
                        "INSERT INTO prices (offering_id,dimension,micros_per_unit,basis,\
                         source_url,taken_at) VALUES (?1,?2,?3,'declared',?4,?5)",
                        rusqlite::params![oid, dim, micros, o.src, &today])?;
                    wrote_px += 1;
                }
            }
            println!("\nwrote {} companies, {wrote_off} ways to buy, {wrote_px} prices",
                     new_prov.len());
            repair::record_run(&con, "price-files", (matched.len() + missed) as i64,
                matched.len() as i64, wrote_px as i64,
                started.elapsed().as_secs_f64(), "")?;
        }
        "boards" => {
            let started = std::time::Instant::now();
            let polite = reqwest::Client::builder()
                .user_agent("pass-index/1.0")
                .timeout(std::time::Duration::from_secs(180))
                .build()?;
            // Several boards refuse a request that does not look like one.
            let browser = reqwest::Client::builder()
                .user_agent(index::boards::BROWSER)
                .timeout(std::time::Duration::from_secs(180))
                .build()?;

            let mut rows: Vec<index::boards::Placement> = Vec::new();
            // A reader that throws produced nothing, which is a fact worth
            // keeping; the rest of the run still happens.
            match polite.get(index::boards::ARC_PRIZE).send().await {
                Ok(r) => match r.json::<serde_json::Value>().await {
                    Ok(d) => rows.extend(index::boards::arc_prize(&d)),
                    Err(e) => println!("!! arc_prize: {e}"),
                },
                Err(e) => println!("!! arc_prize: {e}"),
            }
            match polite.get(index::boards::TTS_ARENA).send().await {
                Ok(r) => match r.json::<serde_json::Value>().await {
                    Ok(d) => rows.extend(index::boards::tts_arena(&d)),
                    Err(e) => println!("!! tts_arena: {e}"),
                },
                Err(e) => println!("!! tts_arena: {e}"),
            }
            match polite.get(index::boards::EPOCH_ZIP).send().await {
                Ok(r) => match r.bytes().await {
                    Ok(b) => match index::boards::epoch(&b) {
                        Ok(p) => rows.extend(p),
                        Err(e) => println!("!! epoch failed: {e}"),
                    },
                    Err(e) => println!("!! epoch failed: {e}"),
                },
                Err(e) => println!("!! epoch failed: {e}"),
            }
            for (suite, name, url, mcol, scol, metric, lower, measurer, home) in
                index::boards::TABLES
            {
                let html = match browser.get(*url).send().await {
                    Ok(r) => r.text().await.unwrap_or_default(),
                    Err(e) => { println!("!! {suite}: {e}"); continue }
                };
                let got = index::boards::table_rows(&html, mcol, scol);
                if got.len() < 8 {
                    println!("!! {suite}: only {} rows, skipped", got.len());
                    continue;
                }
                for (nm, v) in got {
                    let (_, effort) = index::resolve::strip_lanes(&nm);
                    rows.push(index::boards::Placement {
                        suite: (*suite).into(),
                        board: (*name).into(),
                        metric: format!("{metric}{}",
                            effort.map(|e| format!(" ({e})")).unwrap_or_default()),
                        name: nm,
                        value: v,
                        lower_is_better: *lower,
                        source_url: (*url).into(),
                        measurer: (*measurer).into(),
                        home: (*home).into(),
                    });
                }
            }
            match polite.get(feed::OPENROUTER_MODELS).send().await {
                Ok(r) => match r.json::<serde_json::Value>().await {
                    Ok(d) => rows.extend(index::boards::openrouter_benchmarks(&d)),
                    Err(e) => println!("!! openrouter_benchmarks: {e}"),
                },
                Err(e) => println!("!! openrouter_benchmarks: {e}"),
            }

            let mut r = index::resolve::Resolver::from_conn(&con)?;
            let (bound, unbound, boards) = index::boards::rank_and_bind(&rows, &mut r);
            let missed: usize = unbound.iter().map(|(_, c)| c).sum();

            let mut have: Vec<(String, String, String)> = Vec::new();
            {
                let mut q = con.prepare("SELECT entity_id, suite, metric FROM benchmarks")?;
                let mut qr = q.query([])?;
                while let Some(row) = qr.next()? {
                    have.push((row.get(0)?, row.get(1)?, row.get(2)?));
                }
            }
            let fresh = bound.iter()
                .filter(|b| !have.contains(&(b.entity.clone(), b.suite.clone(), b.metric.clone())))
                .count();

            println!("read {} placements from {boards} boards", rows.len());
            println!("  bound to a model the catalogue holds : {}", bound.len());
            println!("  no such model here                   : {missed}");
            println!("  standings that are new               : {fresh}");
            println!();
            let mut per: Vec<(String, usize)> = Vec::new();
            for b in &bound {
                match per.iter_mut().find(|(s, _)| *s == b.suite) {
                    Some(p) => p.1 += 1,
                    None => per.push((b.suite.clone(), 1)),
                }
            }
            per.sort_by(|a, b| b.1.cmp(&a.1));
            for (s, n) in &per {
                println!("     {s:<22} {n:4} bound");
            }
            println!();
            println!("  commonest unbound names:");
            let mut worst = unbound.clone();
            worst.sort_by(|a, b| b.1.cmp(&a.1));
            for (n, c) in worst.iter().take(12) {
                println!("     {:<46} x{c}", n.chars().take(46).collect::<String>());
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let today: String =
                // The night's collection date, not the benchmarks table's
                // own maximum: that maximum is a value only this job writes,
                // so stamping from it froze every leaderboard page at the
                // date of the first crawl, forever.
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let n = index::boards::write_standings(&con, &bound, &today)?;
            println!("\nwrote {n} standings");
            repair::record_run(&con, "boards", rows.len() as i64, bound.len() as i64,
                n as i64, started.elapsed().as_secs_f64(), "")?;
        }
        "dvc" => {
            let (rows, skipped) = index::prose::dvc_portfolio(&con)?;
            let held: Vec<&index::prose::PortfolioRow> =
                rows.iter().filter(|r| r.held.is_some()).collect();
            let total = rows.len() + skipped.len();
            println!("the fund's own filter names {total} companies");
            println!("  not a company             : {}  ({})",
                     skipped.len(), skipped.join(", "));
            println!("  already in the catalogue  : {}  ({})", held.len(),
                     held.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", "));
            println!("  new to us                 : {}", rows.len() - held.len());
            println!("  with a former name        : {}",
                     rows.iter().filter(|r| r.was.is_some()).count());
            println!("  since acquired            : {}",
                     rows.iter().filter(|r| r.acquired_by.is_some()).count());
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let added = index::prose::write_portfolio(&con, &rows)?;
            let n: i64 = con.query_row(
                "SELECT COUNT(*) FROM investments WHERE fund_id=?1",
                [index::prose::DVC_FUND.0], |r| r.get(0))?;
            println!("\nadded {added} companies; the fund now shows {n} investments");
        }
        "startups" => {
            let limit = args.iter().position(|a| a == "--limit")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let (read, found, dry) = index::prose::read_rounds(&con, limit).await?;
            println!("read {read} companies");
            println!("  rounds found for : {}", found.len());
            println!("  nothing to read  : {}", dry.len());
            println!();
            let mut top: Vec<&index::prose::Raised> = found.iter().collect();
            top.sort_by_key(|f| -f.total);
            for f in top.into_iter().take(25) {
                let name: String = f.name.chars().take(26).collect();
                println!("   {:<26} ${} across {} round{}", name,
                         index::prose::with_commas(f.total), f.rounds,
                         if f.rounds == 1 { "" } else { "s" });
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            index::prose::write_rounds(&con, &found)?;
            println!("\nmarked {} companies as venture-funded", found.len());
        }
        "funds" => {
            let led = index::prose::read_investors(&con).await?;
            println!("investments found : {}", led.edges.len());
            println!("  from Y Combinator saying so : {}", led.yc);
            println!("  read out of {} articles      : {}", led.read,
                     led.edges.len() - led.yc);
            println!("  distinct funds              : {}", led.funds.len());
            println!();
            let mut tally: Vec<(&str, usize)> = Vec::new();
            for ((f, _), _) in &led.edges {
                match tally.iter_mut().find(|(k, _)| k == f) {
                    Some(t) => t.1 += 1,
                    None => tally.push((f, 1)),
                }
            }
            tally.sort_by_key(|(_, n)| -(*n as i64));
            for (f, n) in tally.into_iter().take(22) {
                let spelling = &led.funds.iter().find(|(k, _)| k == f).unwrap().1;
                let spelling: String = spelling.chars().take(32).collect();
                println!("   {spelling:<32} {n}");
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let made = index::prose::write_investors(&con, &led)?;
            let n: i64 = con.query_row("SELECT COUNT(*) FROM investments", [],
                                       |r| r.get(0))?;
            println!("\n{made} funds added, {n} investments recorded");
        }
        "enrich" => {
            let Some(fund) = args.iter().position(|a| a == "--fund")
                .and_then(|i| args.get(i + 1)) else {
                eprintln!("repair: enrich needs --fund <name>");
                std::process::exit(2);
            };
            let Some(g) = index::prose::gather(&con, fund).await? else {
                println!("no such fund");
                std::process::exit(1);
            };
            if let Some(note) = &g.note { println!("{note}"); }
            println!("{} companies in {}'s portfolio", g.portfolio, fund);
            for (k, v) in &g.tally {
                println!("   {k:<20} {v}");
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            index::prose::write_blurbs(&con, &g.writes)?;
            println!("\nwrote {} descriptions", g.writes.len());
        }
        "yc" => {
            let (b, notes) = index::prose::yc_batch(&con).await?;
            for n in &notes { println!("{n}"); }
            println!("Y Combinator lists {} AI companies", b.listed);
            println!("  no longer going, left out : {}", b.dead);
            println!("  already in the catalogue  : {}", b.matched.len());
            println!("  new to us                 : {}", b.fresh.len());
            let mut by_year: Vec<(String, usize)> = Vec::new();
            for c in &b.fresh {
                let batch = c["batch"].as_str().unwrap_or("?");
                let year = batch.split_whitespace().last().unwrap_or("?").to_string();
                match by_year.iter_mut().find(|(y, _)| *y == year) {
                    Some(e) => e.1 += 1,
                    None => by_year.push((year, 1)),
                }
            }
            by_year.sort();
            let tail = by_year.len().saturating_sub(6);
            println!("  newest batches            : {}", by_year[tail..].iter()
                .map(|(y, n)| format!("{y}×{n}"))
                .collect::<Vec<_>>().join(", "));
            println!();
            for c in b.fresh.iter().take(12) {
                let name: String = c["name"].as_str().unwrap_or("").chars().take(26).collect();
                let one: String = c["one_liner"].as_str().unwrap_or("").chars().take(44).collect();
                println!("   {:<26} {:<12} {}", name, c["batch"].as_str().unwrap_or(""), one);
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let (added, touched) = index::prose::write_batch(&con, &b)?;
            println!("\nadded {added} companies, marked {touched} already here");
        }
        "discover" => {
            let limit = args.iter().position(|a| a == "--limit")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let sw = index::prose::discover(&con, limit).await?;
            for n in &sw.notes { println!("{n}"); }
            println!("companies the sources list : {}", sw.offered);
            println!("  already in the catalogue : {}", sw.offered - sw.fresh);
            println!("  new to us                : {}", sw.fresh);
            println!("  of the new ones, a round we can read: {}", sw.found.len());
            println!();
            let mut top: Vec<&index::prose::Unheard> = sw.found.iter().collect();
            top.sort_by_key(|f| -f.total);
            for f in top.into_iter().take(25) {
                let name: String = f.name.chars().take(28).collect();
                println!("   {:<28} ${} across {}", name,
                         index::prose::with_commas(f.total), f.rounds);
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let added = index::prose::write_unheard(&con, &sw.found)?;
            println!("\nadded {added} companies");
        }
        "licences" => {
            let (todo, found) = index::hands::read_licences(&con).await?;
            println!("{todo} models to resolve");
            println!("{} licences read", found.len());
            let mut tally: Vec<(&str, usize)> = Vec::new();
            for f in &found {
                match tally.iter_mut().find(|(k, _)| *k == f.licence) {
                    Some(t) => t.1 += 1,
                    None => tally.push((&f.licence, 1)),
                }
            }
            tally.sort_by_key(|(_, n)| -(*n as i64));
            for (l, n) in tally.iter().take(12) {
                println!("   {l:<22} {n}");
            }
            let mut top: Vec<&index::hands::Licence> = found.iter().collect();
            top.sort_by_key(|f| -f.sellers);
            for f in top.into_iter().take(25) {
                let name: String = f.name.chars().take(34).collect();
                let repo: String = f.repo.chars().take(40).collect();
                println!("   {:<34} {:<40} {:<16} {} sellers", name, repo, f.licence, f.sellers);
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            index::hands::write_licences(&con, &found)?;
            println!("\n{} licences written", found.len());
        }
        "retire" => {
            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let n = repair::retire(&con, &today, apply)?;
            let s = repair::expire_standings(&con, &today, apply)?;
            if !apply {
                println!("\ndry run; nothing written");
            } else {
                println!("\n{n} offerings marked stale, {s} frozen standings removed");
            }
        }
        "people" => {
            let (read, found) = index::prose::read_people(&con).await?;
            println!("companies with an article : {read}");
            println!("  people found            : {}", found.len());
            for n in found.iter().take(12) {
                println!("   {:<28} {:<11} {}", n.provider.chars().take(28).collect::<String>(),
                         n.field, n.names.join(", "));
            }
            if !apply {
                println!("\ndry run; nothing written");
                return Ok(());
            }
            let today: String =
                con.query_row("SELECT MAX(taken_at) FROM prices", [], |r| r.get(0))?;
            let wrote = index::prose::write_people(&con, &found, &today)?;
            println!("\nwrote {wrote} facts");
        }
        "supply" => {
            let Some(dir) = args.iter().position(|a| a == "--from")
                .and_then(|i| args.get(i + 1).cloned()) else {
                eprintln!("repair: supply needs --from <dir>");
                std::process::exit(2);
            };
            let pen_path = args.iter().position(|a| a == "--pen")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| "/data/quarantine.db".into());
            let pen = if std::path::Path::new(&pen_path).exists() {
                Some(common::db::open(&pen_path)?)
            } else {
                None
            };
            let t = index::supply::consume(&con, pen.as_ref(), &dir, apply)?;
            println!("supplier files: {} new, {} unchanged and skipped", t.files, t.skipped_files);
            println!("  observations read           : {}", t.read);
            println!("  prices, stated exactly      : {}", t.prices);
            println!("  kept as evidence            : {}", t.evidence);
            println!("  standings                   : {}", t.standings);
            println!("  candidates into the pen     : {}", t.candidates);
            println!("  dropped (unbound or noise)  : {}", t.dropped);
            if !apply {
                println!("\ndry run; nothing written");
            }
        }
        other => {
            eprintln!("repair: no job called {other}");
            std::process::exit(2);
        }
    }
    Ok(())
}

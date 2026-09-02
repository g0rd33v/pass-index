//! Import a curated source file into the catalogue. The file is a human's
//! work — entities and alias bindings in it are deliberate acts, so identity
//! rule 1 is satisfied by authorship, not by quarantine. Prices and metrics
//! land as declared components, each carrying its own source_url and
//! taken_at; a re-import of an unchanged file writes nothing.
//!
//! Usage: import <db> <file.json>
//!
//! File shape:
//! {
//!   "providers": [{"id","name","url","kind","notes"?}],
//!   "entities":  [{"id","register","name","input_kind","output_kind",
//!                  "maker"?,"family"?,"version"?,"derived_from"?,"attrs"?}],
//!   "aliases":   [{"source","alias","entity"}],
//!   "offerings": [{"entity","provider","way","variant"?,
//!                  "components":[{"dimension","source_url","taken_at",
//!                                 "micros_per_unit" | "usd_per_unit"}],
//!                  "metrics":[{"metric","value","source_url","taken_at"}]?}]
//! }
//! usd_per_unit is USD per one unit of the dimension (for mtok_* that unit is
//! a million tokens); it becomes integer micro-USD at the door.

use anyhow::{bail, Context, Result};
use index::{Entity, Index, Provider};
use serde_json::Value;

fn req<'a>(v: &'a Value, key: &str, what: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .with_context(|| format!("{what} needs \"{key}\": {v}"))
}

fn main() -> Result<()> {
    let (db, file) = match (std::env::args().nth(1), std::env::args().nth(2)) {
        (Some(db), Some(file)) => (db, file),
        _ => bail!("usage: import <db> <file.json>"),
    };
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    let ix = Index::open(&db)?;

    for p in doc["providers"].as_array().into_iter().flatten() {
        ix.upsert_provider(&Provider {
            id: req(p, "id", "provider")?.into(),
            name: req(p, "name", "provider")?.into(),
            url: p["url"].as_str().map(Into::into),
            kind: p["kind"].as_str().map(Into::into),
            notes: p["notes"].as_str().map(Into::into),
        })?;
    }

    let (mut minted, mut kept) = (0, 0);
    for e in doc["entities"].as_array().into_iter().flatten() {
        let id = req(e, "id", "entity")?;
        let entity = Entity {
            id: id.into(),
            register: req(e, "register", "entity")?.into(),
            name: req(e, "name", "entity")?.into(),
            input_kind: req(e, "input_kind", "entity")?.into(),
            output_kind: req(e, "output_kind", "entity")?.into(),
            maker: e["maker"].as_str().map(Into::into),
            family: e["family"].as_str().map(Into::into),
            version: e["version"].as_str().map(Into::into),
            derived_from: e["derived_from"].as_str().map(Into::into),
            attrs: if e["attrs"].is_object() { e["attrs"].to_string() } else { "{}".into() },
        };
        // An entity that already exists keeps its row — an import never
        // rewrites what a human minted earlier.
        match ix.insert_entity(&entity) {
            Ok(()) => minted += 1,
            Err(_) => kept += 1,
        }
    }

    for a in doc["aliases"].as_array().into_iter().flatten() {
        let (source, alias) = (req(a, "source", "alias")?, req(a, "alias", "alias")?);
        ix.bind_alias(source, alias, req(a, "entity", "alias")?)?;
        ix.remove_unmatched(source, alias)?;
    }

    let (mut offerings, mut appended, mut unchanged) = (0, 0, 0);
    for o in doc["offerings"].as_array().into_iter().flatten() {
        let comps = o["components"]
            .as_array()
            .filter(|c| !c.is_empty())
            .with_context(|| format!("offering needs components: {o}"))?;
        let mut seen_at = "";
        for c in comps {
            seen_at = seen_at.max(req(c, "taken_at", "component")?);
        }
        let off = ix.upsert_offering(
            req(o, "entity", "offering")?,
            req(o, "provider", "offering")?,
            req(o, "way", "offering")?,
            o["variant"].as_str().unwrap_or(""),
            seen_at,
        )?;
        offerings += 1;
        for c in comps {
            let micros = match (&c["micros_per_unit"], &c["usd_per_unit"]) {
                (Value::Number(n), _) if n.is_i64() => n.as_i64().unwrap(),
                (_, Value::Number(n)) => (n.as_f64().unwrap() * 1e6).round() as i64,
                _ => bail!("component needs micros_per_unit or usd_per_unit: {c}"),
            };
            let done = ix.add_price_if_changed(
                off,
                req(c, "dimension", "component")?,
                micros,
                req(c, "source_url", "component")?,
                req(c, "taken_at", "component")?,
            )?;
            if done { appended += 1 } else { unchanged += 1 }
        }
        for m in o["metrics"].as_array().into_iter().flatten() {
            let done = ix.add_metric_if_changed(
                off,
                req(m, "metric", "metric")?,
                m["value"].as_f64().context("metric needs value")?,
                req(m, "source_url", "metric")?,
                req(m, "taken_at", "metric")?,
            )?;
            if done { appended += 1 } else { unchanged += 1 }
        }
    }
    // Suites first: a score without its suite is a number without a question.
    let mut suites = 0;
    for s in doc["suites"].as_array().into_iter().flatten() {
        ix.upsert_suite(
            req(s, "id", "suite")?,
            req(s, "name", "suite")?,
            s["measurer"].as_str(),
            s["url"].as_str(),
            s["metric"].as_str(),
            s["subject"].as_str(),
            s["lower_is_better"].as_bool().unwrap_or(false),
            s["updated"].as_str(),
        )?;
        suites += 1;
    }

    // A leaderboard names models its own way. The binding goes through the
    // alias table exactly like a price source's does; a name that resolves to
    // nothing waits in the queue instead of being guessed at.
    let (mut scored, mut moved, mut held) = (0, 0, 0);
    for b in doc["benchmarks"].as_array().into_iter().flatten() {
        let suite = req(b, "suite", "benchmark")?;
        let printed = req(b, "model_as_printed", "benchmark")?;
        let source = format!("bench_{suite}");
        let taken_at = req(b, "taken_at", "benchmark")?;
        let Some(entity) = ix.resolve(&source, printed)? else {
            ix.upsert_unmatched(&source, printed, &b.to_string(), taken_at)?;
            held += 1;
            continue;
        };
        scored += 1;
        if ix.add_benchmark_if_changed(
            &entity,
            suite,
            req(b, "metric", "benchmark")?,
            b["value"].as_f64().context("benchmark needs value")?,
            b["rank"].as_i64(),
            b["out_of"].as_i64(),
            req(b, "source_url", "benchmark")?,
            taken_at,
        )? {
            moved += 1;
        }
    }

    let (mut texts, mut described) = (0, 0);
    // A fact about what an entity is applies to the entity, not only to the
    // shelf it was read onto — the same rule the collector follows.
    let mut facts: std::collections::HashMap<String, (Option<String>, Option<String>, Option<i64>)> =
        Default::default();
    for d in doc["docs"].as_array().into_iter().flatten() {
        let subject = req(d, "subject", "doc")?;
        let kind = req(d, "kind", "doc")?;
        let field = d["field"].as_str();
        let text = req(d, "text", "doc")?;
        ix.upsert_doc(subject, kind, field, text, req(d, "source_url", "doc")?,
                      req(d, "taken_at", "doc")?)?;
        texts += 1;
        if kind == "fact" && subject.starts_with("ent_") {
            let slot = facts.entry(subject.to_string()).or_default();
            match field {
                Some("input_kind") => slot.0 = Some(text.to_string()),
                Some("output_kind") => slot.1 = Some(text.to_string()),
                Some("context") => slot.2 = text.parse().ok(),
                _ => {}
            }
        }
    }
    for (subject, (input, output, context)) in facts {
        if ix.set_entity_facts(&subject, input.as_deref(), output.as_deref(), context)? {
            described += 1;
        }
    }
    if suites + scored + held + texts + described > 0 {
        println!(
            "  {suites} suites, {scored} standings bound ({moved} moved), {held} names held, \
             {texts} texts, {described} entities described"
        );
    }

    let swept = ix.drop_empty_offerings()?;
    println!(
        "{file}: {minted} entities minted ({kept} kept), {offerings} offerings, {appended} figures appended, {unchanged} unchanged, {swept} swept"
    );
    Ok(())
}

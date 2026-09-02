//! The human side of identity rule 1: entities are minted deliberately, and
//! aliases are bound by hand. The collector only proposes, via the
//! quarantine queue this tool reads.
//!
//! Usage:
//!   mint <db> list
//!       show the quarantine queue
//!   mint <db> entity '<json>'
//!       create an entity; JSON keys: id, register, name, input_kind,
//!       output_kind (required); maker, family, version, derived_from,
//!       attrs (optional)
//!   mint <db> bind <source> <alias> <entity_id>
//!       bind the alias and clear its quarantine row
//!   mint <db> absorb <source> [limit]
//!       take a source's quarantine queue into the catalogue wholesale
//!   mint <db> tidy
//!       re-read every modality column in the catalogue's vocabulary
//!   mint <db> merge <loser> <survivor> [variant-hint]
//!       fold one entity into another — the same weights minted twice

use index::{Entity, Index};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (db, cmd) = match args.as_slice() {
        [db, cmd, ..] => (db.clone(), cmd.clone()),
        _ => anyhow::bail!("usage: mint <db> list | entity '<json>' | bind <source> <alias> <entity_id>"),
    };
    let ix = Index::open(&db)?;
    match (cmd.as_str(), &args[2..]) {
        ("list", []) => {
            let rows = ix.unmatched()?;
            for (source, alias, payload, last_seen) in &rows {
                let short: String = payload.chars().take(120).collect();
                println!("{source}\t{alias}\t{last_seen}\t{short}");
            }
            println!("{} unmatched", rows.len());
        }
        ("entity", [json]) => {
            let v: serde_json::Value = serde_json::from_str(json)?;
            let req = |key: &str| -> anyhow::Result<String> {
                v[key]
                    .as_str()
                    .map(Into::into)
                    .ok_or_else(|| anyhow::anyhow!("entity JSON needs \"{key}\""))
            };
            let opt = |key: &str| v[key].as_str().map(String::from);
            let e = Entity {
                id: req("id")?,
                register: req("register")?,
                name: req("name")?,
                input_kind: req("input_kind")?,
                output_kind: req("output_kind")?,
                maker: opt("maker"),
                family: opt("family"),
                version: opt("version"),
                derived_from: opt("derived_from"),
                attrs: if v["attrs"].is_object() { v["attrs"].to_string() } else { "{}".into() },
            };
            ix.insert_entity(&e)?;
            println!("minted {}", e.id);
        }
        ("bind", [source, alias, entity_id]) => {
            ix.bind_alias(source, alias, entity_id)?;
            let cleared = ix.remove_unmatched(source, alias)?;
            println!(
                "bound ({source}, {alias}) -> {entity_id}{}",
                if cleared { ", quarantine row cleared" } else { "" }
            );
        }
        ("absorb", [source, rest @ ..]) => {
            let limit: usize = rest.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let (minted, bound, opened, skipped) = ix.absorb(source, limit)?;
            println!(
                "{source}: {minted} entities minted, {bound} aliases bound, \
                 {opened} providers opened, {skipped} skipped"
            );
        }
        ("tidy", []) => {
            let changed = ix.tidy_kinds()?;
            println!("{changed} entities re-read in the catalogue's modality vocabulary");
        }
        ("merge", [loser, survivor, rest @ ..]) => {
            let hint = rest.first().map(String::as_str).unwrap_or("preview");
            let (offs, aliases, standings, texts) = ix.merge_entity(loser, survivor, hint)?;
            println!(
                "{loser} -> {survivor}: {offs} offerings, {aliases} aliases, \
                 {standings} standings, {texts} texts moved"
            );
        }
        _ => anyhow::bail!(
            "usage: mint <db> list | entity '<json>' | bind <source> <alias> <entity_id> \
             | merge <loser> <survivor> [variant-hint]"
        ),
    }
    Ok(())
}

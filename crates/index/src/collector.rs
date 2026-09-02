//! The collector (Stage 2): turns a provider's published listing into
//! catalogue rows. Identity rule 1 holds by construction — a listing whose
//! alias resolves lands as an offering with declared price components; an
//! unknown alias is quarantined in `unmatched_listings` for a human to mint.
//!
//! Parsing is pure and tested here; the `collect` binary does the HTTP.

use crate::Index;
use anyhow::Result;
use serde_json::{json, Value};

/// One priced listing as a source publishes it: the source's own name for the
/// entity, the price components in catalogue dimensions, and the raw facts
/// kept for the quarantine payload.
#[derive(Debug, Clone)]
pub struct Listing {
    pub alias: String,
    /// The offering lane this listing prices ("" for the standard lane,
    /// "batch" for a batch tier the source prices separately).
    pub variant: String,
    /// (dimension, micro-USD per unit)
    pub components: Vec<(String, i64)>,
    pub payload: Value,
}

/// What one collector run did, for the operator's one-line summary.
#[derive(Debug, Default, PartialEq)]
pub struct RunStats {
    pub matched: usize,
    pub appended: usize,
    pub unchanged: usize,
    pub quarantined: usize,
    /// Quarantined listings this source stopped offering, dropped by the run.
    pub pruned: usize,
    /// Components refused because another listing of the same run already
    /// priced that dimension of the same offering differently.
    pub conflicted: usize,
    /// Entities whose stated modalities or context the source corrected.
    pub described: usize,
}

impl std::fmt::Display for RunStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} matched ({} components appended, {} unchanged, {} conflicted), \
             {} quarantined, {} pruned, {} described",
            self.matched, self.appended, self.unchanged, self.conflicted, self.quarantined,
            self.pruned, self.described
        )
    }
}

/// OpenRouter's model list (`GET /api/v1/models`): `data[]` with `id` as the
/// alias and `pricing` in USD per single unit (token, request, image).
pub fn parse_openrouter(body: &Value) -> Vec<Listing> {
    // USD per one token -> integer micro-USD per million tokens.
    let per_mtok = |usd_per_unit: f64| (usd_per_unit * 1e12).round() as i64;
    // USD per one call/image -> integer micro-USD per unit.
    let per_unit = |usd: f64| (usd * 1e6).round() as i64;
    let dims: &[(&str, &str, &dyn Fn(f64) -> i64)] = &[
        ("prompt", "mtok_in", &per_mtok),
        ("completion", "mtok_out", &per_mtok),
        ("input_cache_read", "mtok_cache_read", &per_mtok),
        ("input_cache_write", "mtok_cache_write", &per_mtok),
        ("request", "call", &per_unit),
        ("image", "image", &per_unit),
    ];
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension, to_micros) in dims {
            let rate = m["pricing"][field].as_str().and_then(|s| s.parse::<f64>().ok());
            match rate {
                // A rate can be real and still fall under the smallest unit
                // the catalogue stores: OpenRouter prices an image at
                // $0.000000375, which is nought after rounding to micros. A
                // stored nought prints as "$0" and then never corrects
                // itself, because the next crawl skips the dimension too.
                // Unknown is the honest answer until prices hold nanos.
                Some(usd) if usd > 0.0 && to_micros(usd) > 0 => {
                    components.push((dimension.to_string(), to_micros(usd)))
                }
                _ => {}
            }
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({
                "name": m["name"],
                "pricing": m["pricing"],
                "context_length": m["context_length"],
                // What goes in and what comes out, as the source states it —
                // an entity minted from this listing must not guess them.
                "input_modalities": m["architecture"]["input_modalities"],
                "output_modalities": m["architecture"]["output_modalities"],
                // The source's own sentence about the model — the material an
                // About paragraph is written from, never a remembered one.
                "description": m["description"],
            }),
        });
    }
    out
}

/// OpenRouter's per-model endpoint list (`GET /models/{id}/endpoints`): the
/// same weights served by many upstreams at many prices. The model list gives
/// only the route OpenRouter picks by default, so this is what makes
/// "via CoreWeave" and "via SambaNova" separate ways to buy.
pub fn parse_openrouter_endpoints(body: &Value) -> Vec<Listing> {
    let per_mtok = |v: &Value| -> Option<i64> {
        let usd: f64 = v.as_str()?.parse().ok()?;
        (usd > 0.0).then(|| (usd * 1e12).round() as i64)
    };
    let Some(alias) = body["data"]["id"].as_str() else { return Vec::new() };
    let mut out = Vec::new();
    for e in body["data"]["endpoints"].as_array().into_iter().flatten() {
        // A negative status is OpenRouter deranking or disabling the upstream;
        // its price is no longer a price anyone can pay.
        if e["status"].as_i64().unwrap_or(0) < 0 {
            continue;
        }
        // The tag names the upstream AND the quantisation it serves, and two
        // quantisations of one model are genuinely two products.
        let variant = e["tag"]
            .as_str()
            .or_else(|| e["provider_name"].as_str())
            .unwrap_or_default()
            .to_lowercase();
        let mut components = Vec::new();
        for (field, dimension) in [
            ("prompt", "mtok_in"),
            ("completion", "mtok_out"),
            ("input_cache_read", "mtok_cache_read"),
            ("input_cache_write", "mtok_cache_write"),
        ] {
            if let Some(micros) = per_mtok(&e["pricing"][field]).filter(|m| *m > 0) {
                components.push((dimension.to_string(), micros));
            }
        }
        if variant.is_empty() || components.is_empty() {
            continue;
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant,
            components,
            payload: json!({ "name": body["data"]["name"], "context_length": e["context_length"] }),
        });
    }
    out
}

/// Novita's OpenAI-shaped model list: prices are decimal USD per million
/// tokens, and the record already names the modalities and the context.
pub fn parse_novita(body: &Value) -> Vec<Listing> {
    let per_mtok = |m: &Value, field: &str| -> Option<i64> {
        let s = m["pricing"][field]["price_per_m_decimal"].as_str()?;
        let usd: f64 = s.parse().ok()?;
        (usd > 0.0).then(|| (usd * 1e6).round() as i64)
    };
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension) in [
            ("prompt", "mtok_in"),
            ("completion", "mtok_out"),
            ("input_cache_read", "mtok_cache_read"),
            ("input_cache_write", "mtok_cache_write"),
        ] {
            if let Some(micros) = per_mtok(m, field).filter(|m| *m > 0) {
                components.push((dimension.to_string(), micros));
            }
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({
                "name": m["display_name"],
                "context_length": m["context_size"],
                "input_modalities": m["input_modalities"],
                "output_modalities": m["output_modalities"],
                "description": m["description"],
            }),
        });
    }
    out
}

/// Chutes prints USD per million tokens in `pricing`, and the length it will
/// actually serve in `context_length` (`max_model_len` is what the weights
/// allow, which is not the same promise).
pub fn parse_chutes(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension) in [
            ("prompt", "mtok_in"),
            ("completion", "mtok_out"),
            ("input_cache_read", "mtok_cache_read"),
        ] {
            match m["pricing"][field].as_f64() {
                Some(usd) if usd > 0.0 => {
                    components.push((dimension.to_string(), (usd * 1e6).round() as i64))
                }
                _ => {}
            }
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({
                "name": m["id"],
                "context_length": m["context_length"],
                "input_modalities": m["input_modalities"],
                "output_modalities": m["output_modalities"],
            }),
        });
    }
    out
}

/// Requesty's router quotes USD per ONE token, and states the discount it is
/// currently running separately — the recorded figure is what the list price
/// says, not the promotion.
pub fn parse_requesty(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension) in [
            ("input_price", "mtok_in"),
            ("output_price", "mtok_out"),
            ("cached_price", "mtok_cache_read"),
        ] {
            match m[field].as_f64() {
                Some(usd) if usd > 0.0 => {
                    components.push((dimension.to_string(), (usd * 1e12).round() as i64))
                }
                _ => {}
            }
        }
        // The router sells the same weights through whichever upstream will
        // serve them, at that upstream's price — so the upstream is the lane,
        // and "glm-5.2 via fireworks" is not the same way to buy as
        // "glm-5.2 via nebius".
        let variant = alias.split_once('/').map(|(org, _)| org.to_string()).unwrap_or_default();
        out.push(Listing {
            alias: alias.to_string(),
            variant,
            components,
            payload: json!({
                "name": m["id"],
                "context_length": m["context_window"],
                "description": m["description"],
            }),
        });
    }
    out
}

/// DeepInfra names the unit it charges in — tokens, images, seconds of audio,
/// characters — and quotes it in CENTS. Each pricing type maps to one
/// dimension; `frame_units` is left alone, because a frame is not a unit the
/// catalogue can compare.
pub fn parse_deepinfra(body: &Value) -> Vec<Listing> {
    let cents_per_mtok = |c: f64| (c * 1e10).round() as i64; // cents/token -> micro-USD/Mtok
    let cents_per_unit = |c: f64| (c * 1e4).round() as i64; // cents/unit  -> micro-USD/unit
    let mut out = Vec::new();
    for m in body.as_array().into_iter().flatten() {
        let Some(alias) = m["model_name"].as_str() else { continue };
        if m["deprecated"].as_str().is_some() {
            continue;
        }
        let p = &m["pricing"];
        let mut components = Vec::new();
        let mut push = |dim: &str, v: Option<f64>, f: &dyn Fn(f64) -> i64| {
            // see the note above: a rate that rounds to nought is not free
            if let Some(x) = v.filter(|x| *x > 0.0).filter(|x| f(*x) > 0) {
                components.push((dim.to_string(), f(x)));
            }
        };
        match p["type"].as_str().unwrap_or("") {
            "tokens" => {
                push("mtok_in", p["cents_per_input_token"].as_f64(), &cents_per_mtok);
                push("mtok_out", p["cents_per_output_token"].as_f64(), &cents_per_mtok);
            }
            "input_tokens" => push("mtok_in", p["cents_per_input_token"].as_f64(), &cents_per_mtok),
            "image_units" => push("image", p["cents_per_image_unit"].as_f64(), &cents_per_unit),
            "output_length" => push("second", p["cents_per_output_sec"].as_f64(), &cents_per_unit),
            "input_length" => push("second", p["cents_per_input_sec"].as_f64(), &cents_per_unit),
            "time" => push("second", p["cents_per_sec"].as_f64(), &cents_per_unit),
            "input_character_length" => {
                push("character", p["cents_per_input_chars"].as_f64(), &cents_per_unit)
            }
            _ => {}
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({
                "name": m["model_name"],
                "context_length": m["max_tokens"],
                "kind": m["type"],
                "description": m["description"],
            }),
        });
    }
    out
}

/// SambaNova quotes USD per ONE token as a string, the way OpenAI's own list
/// does, and states the length it serves.
pub fn parse_sambanova(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension) in [("prompt", "mtok_in"), ("completion", "mtok_out")] {
            let rate = m["pricing"][field].as_str().and_then(|s| s.parse::<f64>().ok());
            if let Some(usd) = rate.filter(|u| *u > 0.0) {
                components.push((dimension.to_string(), (usd * 1e12).round() as i64));
            }
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({ "name": m["id"], "context_length": m["context_length"] }),
        });
    }
    out
}

/// Nebius Token Factory publishes a model card per model and a `flavors` list
/// per serving tier: the flavor's own label is the lane, and its price is
/// already USD per million tokens.
pub fn parse_nebius(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body.as_array().into_iter().flatten() {
        if m["status"].as_str() == Some("inactive") {
            continue;
        }
        for f in m["flavors"].as_array().into_iter().flatten() {
            let Some(alias) = f["model_id"].as_str() else { continue };
            let mut components = Vec::new();
            for (field, dimension) in [
                ("input_price_per_million_tokens", "mtok_in"),
                ("output_price_per_million_tokens", "mtok_out"),
                ("cached_input_price_per_million_tokens", "mtok_cache_read"),
            ] {
                if let Some(usd) = f[field].as_f64().filter(|u| *u > 0.0) {
                    components.push((dimension.to_string(), (usd * 1e6).round() as i64));
                }
            }
            // "cheap" is Nebius's own word for its standard lane, so it names
            // no variant; anything else is a lane the catalogue should show.
            let label = f["label"].as_str().unwrap_or("");
            out.push(Listing {
                alias: alias.to_string(),
                variant: if label == "cheap" { String::new() } else { label.to_string() },
                components,
                payload: json!({
                    "name": m["name"],
                    "description": m["description"],
                    "context_length": f["max_model_len"],
                }),
            });
        }
    }
    out
}

/// Hugging Face routes one model to whichever partner serves it and charges
/// the partner's own price, so the partner is the lane — "via novita" and
/// "via together" are two different ways to buy the same weights.
pub fn parse_hf_router(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        for p in m["providers"].as_array().into_iter().flatten() {
            if p["status"].as_str() != Some("live") {
                continue;
            }
            let Some(partner) = p["provider"].as_str() else { continue };
            let mut components = Vec::new();
            for (field, dimension) in [("input", "mtok_in"), ("output", "mtok_out")] {
                if let Some(usd) = p["pricing"][field].as_f64().filter(|u| *u > 0.0) {
                    components.push((dimension.to_string(), (usd * 1e6).round() as i64));
                }
            }
            out.push(Listing {
                alias: alias.to_string(),
                variant: partner.to_string(),
                components,
                payload: json!({
                    "name": m["id"],
                    "context_length": p["context_length"],
                    "input_modalities": m["architecture"]["input_modalities"],
                    "output_modalities": m["architecture"]["output_modalities"],
                }),
            });
        }
    }
    out
}

/// Vercel's gateway quotes USD per ONE token as a string and carries the
/// maker's own description, the context it serves and the modalities.
pub fn parse_vercel(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (field, dimension) in [
            ("input", "mtok_in"),
            ("output", "mtok_out"),
            ("input_cache_read", "mtok_cache_read"),
            ("input_cache_write", "mtok_cache_write"),
        ] {
            let rate = m["pricing"][field].as_str().and_then(|s| s.parse::<f64>().ok());
            if let Some(usd) = rate.filter(|u| *u > 0.0) {
                components.push((dimension.to_string(), (usd * 1e12).round() as i64));
            }
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: json!({
                "name": m["name"],
                "description": m["description"],
                "context_length": m["context_window"],
                "input_modalities": m["modalities"]["input"],
                "output_modalities": m["modalities"]["output"],
            }),
        });
    }
    out
}

/// AWS's public offer file for Bedrock (us-east-1): `products` carry clean
/// attributes (model, provider, feature, inferenceType), `terms.OnDemand`
/// the rate. Only the inference types below are understood; anything else
/// (priority/flex lanes, image/video token counts) is skipped, never guessed.
pub fn parse_bedrock(offer: &Value) -> Vec<Listing> {
    // The offer file is region-scoped (us-east-1), so its rates are the
    // in-region tier — named, because AWS's own pricing page publishes the
    // cross-region tier at other rates for the same models.
    fn dimension(inference_type: &str) -> Option<(&'static str, &'static str)> {
        // (dimension, variant)
        match inference_type {
            "Input tokens" => Some(("mtok_in", "in-region")),
            "Output tokens" => Some(("mtok_out", "in-region")),
            "input tokens batch" => Some(("mtok_in", "in-region batch")),
            "output tokens batch" => Some(("mtok_out", "in-region batch")),
            "Prompt cache read input tokens" => Some(("mtok_cache_read", "in-region")),
            "Prompt cache write input tokens" => Some(("mtok_cache_write", "in-region")),
            _ => None,
        }
    }
    let mut grouped: std::collections::BTreeMap<(String, String), Vec<(String, i64)>> =
        Default::default();
    let mut providers: std::collections::BTreeMap<String, String> = Default::default();
    let empty = serde_json::Map::new();
    for (sku, product) in offer["products"].as_object().unwrap_or(&empty) {
        let attrs = &product["attributes"];
        let feature = attrs["feature"].as_str().unwrap_or("");
        if feature != "On-demand Inference" && feature != "Batch Inference" {
            continue;
        }
        let Some(model) = attrs["model"].as_str().filter(|m| !m.is_empty()) else { continue };
        let Some((dim, variant)) = dimension(attrs["inferenceType"].as_str().unwrap_or("")) else {
            continue;
        };
        // terms.OnDemand[sku] -> the single price dimension's pricePerUnit.
        let Some(term) = offer["terms"]["OnDemand"][sku].as_object().and_then(|t| t.values().next())
        else {
            continue;
        };
        let Some(pd) = term["priceDimensions"].as_object().and_then(|p| p.values().next()) else {
            continue;
        };
        let Some(usd) = pd["pricePerUnit"]["USD"].as_str().and_then(|s| s.parse::<f64>().ok())
        else {
            continue;
        };
        let per_mtok = match pd["unit"].as_str().unwrap_or("") {
            "1K tokens" => usd * 1e3,
            "1M tokens" => usd,
            _ => continue,
        };
        if per_mtok <= 0.0 {
            continue;
        }
        grouped
            .entry((model.to_string(), variant.to_string()))
            .or_default()
            .push((dim.to_string(), (per_mtok * 1e6).round() as i64));
        if let Some(p) = attrs["provider"].as_str() {
            providers.insert(model.to_string(), p.to_string());
        }
    }
    grouped
        .into_iter()
        .map(|((model, variant), components)| Listing {
            payload: json!({"name": model, "maker": providers.get(&model)}),
            alias: model,
            variant,
            components: unambiguous(components),
        })
        .collect()
}

/// A dimension the source prices twice with two different rates is ambiguous
/// — drop it rather than let history flap between the two on every run.
fn unambiguous(components: Vec<(String, i64)>) -> Vec<(String, i64)> {
    let mut seen: std::collections::BTreeMap<String, i64> = Default::default();
    let mut conflicted: std::collections::BTreeSet<String> = Default::default();
    for (dim, micros) in components {
        match seen.get(&dim) {
            Some(prev) if *prev != micros => {
                conflicted.insert(dim);
            }
            _ => {
                seen.insert(dim, micros);
            }
        }
    }
    seen.into_iter().filter(|(d, _)| !conflicted.contains(d)).collect()
}

/// Azure's Retail Prices API for Foundry Models: meter names are word soup
/// ("gpt aud 0828 txt Inp DZone"). The grammar is a whitelist: direction,
/// cache, batch and tier words are consumed as modifiers; a modality word
/// standing immediately before the direction word is that meter's modality;
/// every leftover word IS the model alias exactly as Azure prints it, dates
/// and "prvw" included, because those name the deployment.
///
/// Only global-tier token meters are read; a regional or data-zone tier is a
/// different price and is skipped rather than mixed in.
pub fn parse_azure(items: &[Value]) -> Vec<Listing> {
    let mut grouped: std::collections::BTreeMap<(String, String), Vec<(String, i64)>> =
        Default::default();
    let mut makers: std::collections::BTreeMap<String, String> = Default::default();
    for it in items {
        let per_unit_tokens = match it["unitOfMeasure"].as_str().unwrap_or("") {
            "1K" => 1e3,
            "1M" => 1e6,
            _ => continue,
        };
        let Some(sku) = it["skuName"].as_str() else { continue };
        let Some(usd) = it["retailPrice"].as_f64().filter(|p| *p > 0.0) else { continue };

        let words: Vec<&str> = sku.split_whitespace().collect();
        let (mut dir_at, mut cached, mut batch, mut global, mut skip) =
            (None, false, false, false, false);
        let mut lane = None;
        let mut modality = None;
        let mut name: Vec<(usize, &str)> = Vec::new();
        // Word positions consumed as modifiers; the modality search steps
        // over them to reach the direction word.
        let mut modifiers: std::collections::BTreeSet<usize> = Default::default();
        for (i, w) in words.iter().enumerate() {
            if !matches!(
                w.to_ascii_lowercase().as_str(),
                "inp" | "inpt" | "input" | "in" | "outp" | "output" | "out" | "opt"
            ) {
                modifiers.insert(i);
            }
            match w.to_ascii_lowercase().as_str() {
                "inp" | "inpt" | "input" | "in" => dir_at = dir_at.or(Some((i, "in"))),
                "outp" | "output" | "out" | "opt" => dir_at = dir_at.or(Some((i, "out"))),
                "cchd" | "cached" | "cd" => cached = true,
                "batch" => batch = true,
                "glbl" | "gl" | "global" => global = true,
                // Regional and data-zone tiers are different prices on purpose.
                "regnl" | "regn" | "rgnl" | "dzone" | "dzn" | "dz" | "datazone" => skip = true,
                // Azure's lane words: priority processing, and the context
                // tier it prices apart. "Std" and the short-context tier are
                // the base lane the unmarked meters already price.
                "pp" => lane = Some("priority"),
                "longco" | "l" => lane = Some("long-context"),
                "shortco" | "std" => {}
                "tokens" | "1m" | "1k" => {}
                _ => {
                    modifiers.remove(&i);
                    name.push((i, w));
                }
            }
        }
        let (Some((dir_at, dir)), true) = (dir_at, global && !skip) else { continue };
        if name.is_empty() {
            continue;
        }
        // The modality word sits next to the direction word — before it
        // ("txt Inp") or after it ("in cd img") — and belongs to the meter,
        // not to the model's name.
        // The modality word is the one nearest the direction word on either
        // side, stepping over modifiers ("Image 2 img Batch cd inp Gl"). A
        // name word in between ends the search: in "gpt aud 0828 Inp" the
        // aud names the model, not the meter.
        let mut probe = |mut at: usize, step: isize| -> Option<(usize, &'static str)> {
            loop {
                at = at.checked_add_signed(step)?;
                let w = words.get(at)?;
                if let Some(m) = as_modality(w) {
                    return Some((at, m));
                }
                if !modifiers.contains(&at) {
                    return None;
                }
            }
        };
        if let Some((at, found)) = probe(dir_at, -1).or_else(|| probe(dir_at, 1)) {
            if let Some(pos) = name.iter().position(|(i, _)| *i == at) {
                name.remove(pos);
            }
            modality = Some(found);
        }
        let mut name: Vec<&str> = name.into_iter().map(|(_, w)| w).collect();
        let modality = modality
            .or_else(|| modality_after_realtime(&mut name))
            .or_else(|| modality_in_hyphenated_id(&mut name));
        let alias = name.join(" ");
        let modality = modality.unwrap_or_else(|| native_modality(&alias));
        let dimension = match (dir, cached) {
            ("in", false) => "mtok_in",
            ("in", true) => "mtok_cache_read",
            ("out", false) => "mtok_out",
            _ => continue, // cached output is not a thing we know
        };
        let dimension = match modality {
            "text" => dimension.to_string(),
            other => format!("{dimension}_{other}"),
        };
        let variant = match (batch, lane) {
            (true, Some(l)) => format!("{l} batch"),
            (true, None) => "batch".to_string(),
            (false, Some(l)) => l.to_string(),
            (false, None) => String::new(),
        };
        if let Some(product) = it["productName"].as_str() {
            makers.insert(alias.clone(), product.to_string());
        }
        grouped
            .entry((alias, variant))
            .or_default()
            .push((dimension, (usd * 1e6 / per_unit_tokens * 1e6).round() as i64));
    }
    grouped
        .into_iter()
        .map(|((alias, variant), components)| Listing {
            // Azure's product name is the only place the meter says whose
            // model this is ("Azure Grok Models", "MAI Models").
            payload: json!({"name": alias, "product": makers.get(&alias)}),
            alias,
            variant,
            components: unambiguous(components),
        })
        .collect()
}

/// "gpt rt img mini" — a realtime model whose modality sits right after the
/// realtime marker, with the size after it.
fn modality_after_realtime(model_words: &mut Vec<&str>) -> Option<&'static str> {
    if let Some(at) = model_words
        .iter()
        .position(|w| matches!(w.to_ascii_lowercase().as_str(), "rt" | "rtime" | "realtime"))
    {
        if let Some(modality) = model_words.get(at + 1).and_then(|w| as_modality(w)) {
            model_words.remove(at + 1);
            return Some(modality);
        }
    }
    // The same shape inside one hyphenated id: gpt-4o-rt-aud-0603.
    let (idx, seg_at, modality) = model_words.iter().enumerate().find_map(|(i, w)| {
        let segs: Vec<&str> = w.split('-').collect();
        let at = segs
            .iter()
            .position(|s| matches!(s.to_ascii_lowercase().as_str(), "rt" | "rtime" | "realtime"))?;
        let modality = as_modality(segs.get(at + 1)?)?;
        Some((i, at + 1, modality))
    })?;
    let kept: Vec<&str> = model_words[idx]
        .split('-')
        .enumerate()
        .filter(|(j, _)| *j != seg_at)
        .map(|(_, s)| s)
        .collect();
    model_words[idx] = Box::leak(kept.join("-").into_boxed_str());
    Some(modality)
}

/// "gpt-4o-aud-0603-txt" and "gpt4omini-rt-txt1217" — the modality is the
/// LAST hyphen-separated segment of the id, alone or glued to a date. Only
/// the last one: in "gpt-4o-aud-0603" the same word names the model itself.
fn modality_in_hyphenated_id(model_words: &mut Vec<&str>) -> Option<&'static str> {
    let (idx, modality) = model_words.iter().enumerate().find_map(|(i, w)| {
        let mut segs = w.split('-');
        let last = segs.next_back()?;
        if segs.next().is_none() {
            return None; // a single-segment word is the name, not a suffix
        }
        let low = last.to_ascii_lowercase();
        let (head, tail) = low.split_at(3.min(low.len()));
        let modality = as_modality(head)?;
        tail.chars().all(|c| c.is_ascii_digit()).then_some((i, modality))
    })?;
    let word = model_words[idx];
    let cut = word.rfind('-').expect("checked above");
    let kept: &'static str = Box::leak(word[..cut].to_string().into_boxed_str());
    model_words[idx] = kept;
    Some(modality)
}

fn as_modality(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "txt" | "text" => Some("text"),
        "aud" | "audio" => Some("audio"),
        "img" | "image" => Some("image"),
        _ => None,
    }
}

/// What a model called "gpt aud mini" or "Embed v4 Img" meters by default.
fn native_modality(alias: &str) -> &'static str {
    let a = alias.to_ascii_lowercase();
    let has = |w: &str| a.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w);
    if has("aud") || has("audio") || has("trscb") || has("tcrb") || has("tts")
        || has("realtime") || has("realtimeprvw") || has("rtime") || has("rt")
    {
        "audio"
    } else if has("img") || has("image") {
        "image"
    } else {
        "text"
    }
}

/// A suffix a source hangs on a model id to sell the same weights another
/// way, and the offering variant it becomes.
const LANE_SUFFIXES: &[(&str, &str)] = &[
    (":batch", "batch"),
    (":thinking", "thinking"),
    (":extended", "extended"),
    (":online", "online"),
    ("-fast", "fast"),
    ("-preview", "preview"),
    (" Latency Optimized", "latency-optimized"),
];

/// Which entity and which lane a listing belongs to (identity rule 2 allows
/// a trusted rule, never similarity). A lane suffix counts only when the
/// alias without it names the SAME entity — so `claude-opus-5-fast` is Opus 5
/// in the fast lane, while `morph-v3-fast`, whose base names nothing, stays a
/// model of its own. Without this the two lanes share one offering and their
/// two prices overwrite each other on every run.
fn bind(ix: &Index, source: &str, l: &Listing) -> Result<Option<(String, String)>> {
    let direct = ix.resolve(source, &l.alias)?;
    for (suffix, lane) in LANE_SUFFIXES {
        let Some(base) = l.alias.strip_suffix(suffix) else { continue };
        let Some(base_entity) = ix.resolve(source, base)? else { continue };
        if direct.is_none() || direct.as_deref() == Some(base_entity.as_str()) {
            return Ok(Some((base_entity, lane.to_string())));
        }
    }
    Ok(direct.map(|e| (e, l.variant.clone())))
}

/// Copy the source's own statement of what an entity is onto the entity, and
/// keep the sentence it came from with its address. Silence in the payload
/// changes nothing: a source that does not mention modalities has not said
/// they are absent.
fn sync_facts(
    ix: &Index,
    entity_id: &str,
    l: &Listing,
    source_url: &str,
    taken_at: &str,
) -> Result<bool> {
    let join = |key: &str| -> Option<String> {
        let list = l.payload.get(key)?.as_array()?;
        let parts: Vec<&str> = list.iter().filter_map(|v| v.as_str()).collect();
        (!parts.is_empty()).then(|| parts.join(" + "))
    };
    // The source's own sentence about the thing is worth keeping even when the
    // same listing states no modality and no context — a price list that only
    // describes is still the maker describing.
    if let Some(text) = l.payload.get("description").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
        ix.upsert_doc(entity_id, "description", None, text, source_url, taken_at)?;
    }
    let input = join("input_modalities");
    let output = join("output_modalities");
    let context = l.payload.get("context_length").and_then(|v| v.as_i64()).filter(|c| *c > 0);
    if input.is_none() && output.is_none() && context.is_none() {
        return Ok(false);
    }
    let changed = ix.set_entity_facts(entity_id, input.as_deref(), output.as_deref(), context)?;
    for (field, text) in [
        ("input_kind", input),
        ("output_kind", output),
        ("context", context.map(|c| c.to_string())),
    ] {
        if let Some(text) = text {
            ix.upsert_doc(entity_id, "fact", Some(field), &text, source_url, taken_at)?;
        }
    }
    Ok(changed)
}

/// Write one source's listings into the catalogue: bound aliases become
/// offerings with declared components, unknown aliases go to quarantine.
/// A listing without a single price component is skipped entirely — the
/// admission rule asks for per-unit billing.
pub fn apply(
    ix: &Index,
    source: &str,
    way: &str,
    source_url: &str,
    taken_at: &str,
    listings: &[Listing],
) -> Result<RunStats> {
    let mut stats = RunStats::default();
    // What this run already wrote, so two aliases that resolve to one
    // offering cannot take turns overwriting each other's price.
    let mut written: std::collections::HashMap<(i64, String), i64> = Default::default();
    for l in listings {
        if l.components.is_empty() {
            continue;
        }
        let bound = bind(ix, source, l)?;
        match bound {
            Some((entity_id, variant)) => {
                stats.matched += 1;
                // A name that now resolves has left the queue by definition.
                ix.remove_unmatched(source, &l.alias)?;
                // The source that sells the thing also states what it is: the
                // modalities it takes and returns, and the context it holds.
                // The catalogue follows the source rather than whatever a
                // curated file assumed on the day the entity was minted.
                // Only the standard lane speaks for the entity: a fast or
                // preview listing states its own context, and letting it
                // write made the entity's facts flip on every run.
                if variant.is_empty() && sync_facts(ix, &entity_id, l, source_url, taken_at)? {
                    stats.described += 1;
                }
                let off = ix.upsert_offering(&entity_id, source, way, &variant, taken_at)?;
                for (dimension, micros) in &l.components {
                    match written.insert((off, dimension.clone()), *micros) {
                        Some(prev) if prev != *micros => {
                            stats.conflicted += 1;
                            written.insert((off, dimension.clone()), prev);
                            continue;
                        }
                        _ => {}
                    }
                    if ix.add_price_if_changed(off, dimension, *micros, source_url, taken_at)? {
                        stats.appended += 1;
                    } else {
                        stats.unchanged += 1;
                    }
                }
            }
            None => {
                stats.quarantined += 1;
                ix.upsert_unmatched(source, &l.alias, &l.payload.to_string(), taken_at)?;
            }
        }
    }
    let seen: Vec<String> = listings.iter().map(|l| l.alias.clone()).collect();
    stats.pruned = ix.prune_unmatched(source, &seen)?;
    Ok(stats)
}

/// Today as `YYYY-MM-DD` UTC — the `taken_at` of everything a run writes.
pub fn today_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs();
    let days = (secs / 86_400) as i64;
    // Civil-from-days (Howard Hinnant's algorithm), valid far beyond our use.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Entity, Provider};

    fn fixture() -> Value {
        json!({"data": [
            {"id": "anthropic/claude-opus-5", "name": "Claude Opus 5",
             "context_length": 500000,
             "pricing": {"prompt": "0.000005", "completion": "0.000025",
                          "request": "0", "image": "0.02",
                          "input_cache_read": "0.0000005"}},
            {"id": "unknown/new-model", "name": "Brand New",
             "context_length": 8192,
             "pricing": {"prompt": "0.0000001", "completion": "0.0000002"}},
            {"id": "free/model", "name": "Free", "pricing": {"prompt": "0", "completion": "0"}}
        ]})
    }

    fn store() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().unwrap();
        let ix = Index::open(dir.path().join("i.db").to_str().unwrap()).unwrap();
        ix.upsert_provider(&Provider {
            id: "prov_openrouter".into(),
            name: "OpenRouter".into(),
            ..Default::default()
        })
        .unwrap();
        ix.insert_entity(&Entity {
            id: "ent_claude-opus-5".into(),
            register: "model".into(),
            name: "Claude Opus 5".into(),
            input_kind: "text".into(),
            output_kind: "text".into(),
            attrs: "{}".into(),
            ..Default::default()
        })
        .unwrap();
        ix.bind_alias("prov_openrouter", "anthropic/claude-opus-5", "ent_claude-opus-5")
            .unwrap();
        (dir, ix)
    }

    #[test]
    fn openrouter_pricing_lands_in_catalogue_dimensions() {
        // pass-index stage 2: USD-per-token strings become micro-USD per mtok.
        let listings = parse_openrouter(&fixture());
        assert_eq!(listings.len(), 3);
        let opus = &listings[0];
        assert_eq!(opus.alias, "anthropic/claude-opus-5");
        assert!(opus.components.contains(&("mtok_in".into(), 5_000_000)));
        assert!(opus.components.contains(&("mtok_out".into(), 25_000_000)));
        assert!(opus.components.contains(&("mtok_cache_read".into(), 500_000)));
        assert!(opus.components.contains(&("image".into(), 20_000)));
        // zero rates are absence, never a component
        assert!(!opus.components.iter().any(|(d, _)| d == "call"));
        assert!(listings[2].components.is_empty());
    }

    #[test]
    fn bound_alias_becomes_offering_unknown_goes_to_quarantine() {
        // pass-index stage 2: identity rule 1 — the collector never mints.
        let (_d, ix) = store();
        let listings = parse_openrouter(&fixture());
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-24", &listings)
            .unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.appended, 4);
        assert_eq!(s.quarantined, 1);
        assert_eq!(ix.count("entities").unwrap(), 1);
        assert_eq!(ix.count("offerings").unwrap(), 1);
        let q = ix.unmatched().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].1, "unknown/new-model");
    }

    #[test]
    fn a_repeat_run_over_stable_prices_writes_nothing() {
        // pass-index stage 2: history records movement, never repetition.
        let (_d, ix) = store();
        let listings = parse_openrouter(&fixture());
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-24", &listings).unwrap();
        let before = ix.count("prices").unwrap();
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
            .unwrap();
        assert_eq!(s.appended, 0);
        assert_eq!(s.unchanged, 4);
        assert_eq!(ix.count("prices").unwrap(), before);
    }

    #[test]
    fn a_price_move_appends_and_supersedes() {
        // pass-index stage 2: the changed component lands, the old one stays history.
        let (_d, ix) = store();
        let listings = parse_openrouter(&fixture());
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-24", &listings).unwrap();
        let mut moved = listings.clone();
        for c in &mut moved[0].components {
            if c.0 == "mtok_in" {
                c.1 = 4_500_000;
            }
        }
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &moved)
            .unwrap();
        assert_eq!(s.appended, 1);
        let off = ix.upsert_offering("ent_claude-opus-5", "prov_openrouter", "aggregator", "", "2026-08-25").unwrap();
        let now = ix.current_price(off).unwrap();
        let mtok_in = now.iter().find(|c| c.dimension == "mtok_in").unwrap();
        assert_eq!(mtok_in.micros_per_unit, 4_500_000);
        assert_eq!(mtok_in.taken_at, "2026-08-25");
    }

    #[test]
    fn binding_clears_the_quarantine_row() {
        // pass-index stage 2: mint by hand, then the collector picks it up.
        let (_d, ix) = store();
        let listings = parse_openrouter(&fixture());
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-24", &listings).unwrap();
        ix.insert_entity(&Entity {
            id: "ent_new".into(),
            register: "model".into(),
            name: "Brand New".into(),
            input_kind: "text".into(),
            output_kind: "text".into(),
            attrs: "{}".into(),
            ..Default::default()
        })
        .unwrap();
        ix.bind_alias("prov_openrouter", "unknown/new-model", "ent_new").unwrap();
        // The binding alone is enough: the next run clears the queue row.
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
            .unwrap();
        assert_eq!(s.matched, 2);
        assert_eq!(s.quarantined, 0);
        assert_eq!(ix.unmatched().unwrap().len(), 0);
    }

    #[test]
    fn a_batch_listing_is_the_bound_entity_in_the_batch_lane() {
        // pass-index stage 2: ":batch" = same weights, offering variant "batch".
        let (_d, ix) = store();
        let body = json!({"data": [
            {"id": "anthropic/claude-opus-5:batch", "name": "Claude Opus 5 (batch)",
             "pricing": {"prompt": "0.0000025", "completion": "0.0000125"}}
        ]});
        let listings = parse_openrouter(&body);
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-24", &listings)
            .unwrap();
        assert_eq!(s.matched, 1);
        assert_eq!(s.quarantined, 0);
        assert_eq!(ix.count("entities").unwrap(), 1);
        let views = ix.offerings_of("ent_claude-opus-5").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].variant, "batch");
    }

    #[test]
    fn bedrock_offer_becomes_grouped_listings() {
        // pass-index stage 2: clean attributes -> one listing per model+lane.
        let offer = json!({
            "products": {
                "SKU1": {"attributes": {"feature": "On-demand Inference", "model": "Claude 3 Haiku",
                          "provider": "Anthropic", "inferenceType": "Input tokens"}},
                "SKU2": {"attributes": {"feature": "On-demand Inference", "model": "Claude 3 Haiku",
                          "provider": "Anthropic", "inferenceType": "Output tokens"}},
                "SKU3": {"attributes": {"feature": "Batch Inference", "model": "Claude 3 Haiku",
                          "provider": "Anthropic", "inferenceType": "input tokens batch"}},
                "SKU4": {"attributes": {"feature": "On-demand Inference", "model": "Claude 3 Haiku",
                          "provider": "Anthropic", "inferenceType": "Input tokens priority"}},
                "SKU5": {"attributes": {"feature": "Provisioned Throughput Inference - 1 month",
                          "model": "Claude 3 Haiku", "provider": "Anthropic", "inferenceType": "Input tokens"}}
            },
            "terms": {"OnDemand": {
                "SKU1": {"SKU1.X": {"priceDimensions": {"a": {"unit": "1K tokens", "pricePerUnit": {"USD": "0.00025"}}}}},
                "SKU2": {"SKU2.X": {"priceDimensions": {"a": {"unit": "1K tokens", "pricePerUnit": {"USD": "0.00125"}}}}},
                "SKU3": {"SKU3.X": {"priceDimensions": {"a": {"unit": "1M tokens", "pricePerUnit": {"USD": "0.125"}}}}},
                "SKU4": {"SKU4.X": {"priceDimensions": {"a": {"unit": "1K tokens", "pricePerUnit": {"USD": "0.001"}}}}},
                "SKU5": {"SKU5.X": {"priceDimensions": {"a": {"unit": "1K tokens", "pricePerUnit": {"USD": "9.0"}}}}}
            }}
        });
        let listings = parse_bedrock(&offer);
        assert_eq!(listings.len(), 2); // in-region lane + its batch lane; priority and PT skipped
        let std_lane = listings.iter().find(|l| l.variant == "in-region").unwrap();
        assert_eq!(std_lane.alias, "Claude 3 Haiku");
        assert!(std_lane.components.contains(&("mtok_in".into(), 250_000)));
        assert!(std_lane.components.contains(&("mtok_out".into(), 1_250_000)));
        let batch = listings.iter().find(|l| l.variant == "in-region batch").unwrap();
        assert_eq!(batch.components, vec![("mtok_in".into(), 125_000)]);
    }

    #[test]
    fn azure_meter_soup_parses_by_whitelist() {
        // pass-index stage 2: modifiers consumed, leftovers are the alias;
        // regional and data-zone tiers are other prices and are skipped.
        let items = vec![
            json!({"skuName": "gpt 4.1 Inp glbl", "retailPrice": 0.002, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt 4.1 Outp glbl", "retailPrice": 0.008, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt 4.1 cached Inp glbl", "retailPrice": 0.0005, "unitOfMeasure": "1K"}),
            json!({"skuName": "5.4 nano Batch cd Inp Gl 1M Tokens", "retailPrice": 0.01, "unitOfMeasure": "1M"}),
            json!({"skuName": "gpt 4.1 Inp regnl", "retailPrice": 0.0022, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt 4o 0513 Input Data Zone", "retailPrice": 0.0055, "unitOfMeasure": "1K"}),
            json!({"skuName": "Code-Interpreter-global Session", "retailPrice": 0.03, "unitOfMeasure": "1"}),
        ];
        let listings = parse_azure(&items);
        assert_eq!(listings.len(), 2);
        let gpt41 = listings.iter().find(|l| l.alias == "gpt 4.1").unwrap();
        assert_eq!(gpt41.variant, "");
        assert!(gpt41.components.contains(&("mtok_in".into(), 2_000_000)));
        assert!(gpt41.components.contains(&("mtok_out".into(), 8_000_000)));
        assert!(gpt41.components.contains(&("mtok_cache_read".into(), 500_000)));
        let nano = listings.iter().find(|l| l.alias == "5.4 nano").unwrap();
        assert_eq!(nano.variant, "batch");
        assert_eq!(nano.components, vec![("mtok_cache_read".into(), 10_000)]);
    }

    #[test]
    fn azure_prices_audio_and_text_tokens_on_their_own_dimensions() {
        // pass-index stage 2: one audio model, two meters, two units — the
        // gap that kept 56 meters out of the catalogue.
        let items = vec![
            json!({"skuName": "gpt aud 0828 Inp glbl", "retailPrice": 0.04, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt aud 0828 Outp glbl", "retailPrice": 0.08, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt aud 0828 txt Inp glbl", "retailPrice": 0.0025, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt aud 0828 txt Outp glbl", "retailPrice": 0.01, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt4o realtime cached audio inp glbl", "retailPrice": 0.02, "unitOfMeasure": "1K"}),
            json!({"skuName": "Image 2 img Inp glbl", "retailPrice": 0.008, "unitOfMeasure": "1K"}),
            json!({"skuName": "Image 2 txt Inp glbl", "retailPrice": 0.005, "unitOfMeasure": "1K"}),
            // the same grammar with the modality on the other side
            json!({"skuName": "gpt img 1.5 out img gl", "retailPrice": 32.0, "unitOfMeasure": "1M"}),
            json!({"skuName": "gpt img 1.5 in cd txt gl", "retailPrice": 1.25, "unitOfMeasure": "1M"}),
        ];
        let listings = parse_azure(&items);
        let aud = listings.iter().find(|l| l.alias == "gpt aud 0828").unwrap();
        // the unmarked meters of an audio model price audio tokens
        assert!(aud.components.contains(&("mtok_in_audio".into(), 40_000_000)));
        assert!(aud.components.contains(&("mtok_out_audio".into(), 80_000_000)));
        assert!(aud.components.contains(&("mtok_in".into(), 2_500_000)));
        assert!(aud.components.contains(&("mtok_out".into(), 10_000_000)));
        let rt = listings.iter().find(|l| l.alias == "gpt4o realtime").unwrap();
        assert_eq!(rt.components, vec![("mtok_cache_read_audio".into(), 20_000_000)]);
        let img = listings.iter().find(|l| l.alias == "Image 2").unwrap();
        assert!(img.components.contains(&("mtok_in_image".into(), 8_000_000)));
        assert!(img.components.contains(&("mtok_in".into(), 5_000_000)));
        let img15 = listings.iter().find(|l| l.alias == "gpt img 1.5").unwrap();
        assert!(img15.components.contains(&("mtok_out_image".into(), 32_000_000)));
        assert!(img15.components.contains(&("mtok_cache_read".into(), 1_250_000)));
    }

    #[test]
    fn a_listing_the_source_stopped_offering_leaves_the_queue() {
        // pass-index stage 2: the queue is what the source offers today.
        let (_d, ix) = store();
        let gone = vec![Listing {
            alias: "vendor/withdrawn".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 1)],
            payload: json!({}),
        }];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &gone).unwrap();
        assert_eq!(ix.unmatched().unwrap().len(), 1);
        let still = vec![Listing {
            alias: "vendor/current".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 1)],
            payload: json!({}),
        }];
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &still)
            .unwrap();
        assert_eq!(s.pruned, 1);
        let q = ix.unmatched().unwrap();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].1, "vendor/current");
    }

    #[test]
    fn a_second_run_the_same_day_prunes_the_old_grammar() {
        // pass-index stage 2: pruning keys on the run's aliases, not the date,
        // so a parser fix clears the names it no longer produces.
        let (_d, ix) = store();
        let old = vec![Listing {
            alias: "5.6 sol ShortCo Std".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 5_000_000)],
            payload: json!({}),
        }];
        apply(&ix, "prov_azure", "cloud", "https://src", "2026-08-25", &old).unwrap();
        let fixed = vec![Listing {
            alias: "5.6 sol".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 5_000_000)],
            payload: json!({}),
        }];
        let s = apply(&ix, "prov_azure", "cloud", "https://src", "2026-08-25", &fixed).unwrap();
        assert_eq!(s.pruned, 1);
        assert_eq!(ix.unmatched().unwrap().len(), 1);
    }

    #[test]
    fn a_pruning_run_leaves_another_source_alone() {
        // pass-index stage 2: one source's run never touches another's queue.
        let (_d, ix) = store();
        ix.upsert_unmatched("prov_azure", "5.4", "{}", "2026-08-01").unwrap();
        let listings = vec![Listing {
            alias: "vendor/x".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 1)],
            payload: json!({}),
        }];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        assert_eq!(ix.unmatched().unwrap().len(), 2);
    }

    #[test]
    fn the_payload_carries_what_goes_in_and_out() {
        // pass-index stage 2: modalities reach the quarantine payload, so a
        // minted entity states them instead of assuming text.
        let body = json!({"data": [{"id": "vendor/vision", "name": "Vision",
            "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text"]},
            "pricing": {"prompt": "0.000001", "completion": "0.000002"}}]});
        let l = &parse_openrouter(&body)[0];
        assert_eq!(l.payload["input_modalities"], json!(["text", "image"]));
        assert_eq!(l.payload["output_modalities"], json!(["text"]));
    }

    #[test]
    fn azure_writes_the_modality_inside_the_name_too() {
        // pass-index stage 2: three meters of one realtime model, three
        // dimensions, one alias — and a hyphenated id keeps its date.
        let items = vec![
            json!({"skuName": "gpt rt aud mini Inp glbl", "retailPrice": 0.02, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt rt txt mini Inp glbl", "retailPrice": 0.001, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt rt img mini Inp glbl", "retailPrice": 0.003, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt-4o-aud-0603-txt Inp glbl", "retailPrice": 0.005, "unitOfMeasure": "1K"}),
            json!({"skuName": "gpt-4o-aud-0603 Inp glbl", "retailPrice": 0.1, "unitOfMeasure": "1K"}),
            json!({"skuName": "5.6 sol ShortCo Std Inp glbl", "retailPrice": 4.0, "unitOfMeasure": "1M"}),
            json!({"skuName": "5.6 sol LongCo Std Inp glbl", "retailPrice": 8.0, "unitOfMeasure": "1M"}),
            json!({"skuName": "5.6 sol ShortCo PP Inp glbl", "retailPrice": 6.0, "unitOfMeasure": "1M"}),
        ];
        let listings = parse_azure(&items);
        let mini = listings.iter().find(|l| l.alias == "gpt rt mini").unwrap();
        assert!(mini.components.contains(&("mtok_in_audio".into(), 20_000_000)));
        assert!(mini.components.contains(&("mtok_in".into(), 1_000_000)));
        assert!(mini.components.contains(&("mtok_in_image".into(), 3_000_000)));
        let aud = listings.iter().find(|l| l.alias == "gpt-4o-aud-0603").unwrap();
        assert!(aud.components.contains(&("mtok_in".into(), 5_000_000)));
        assert!(aud.components.contains(&("mtok_in_audio".into(), 100_000_000)));
        // context tier and priority processing are lanes, not other models
        let lanes: Vec<&str> = listings
            .iter()
            .filter(|l| l.alias == "5.6 sol")
            .map(|l| l.variant.as_str())
            .collect();
        assert!(lanes.contains(&""), "short context is the base lane: {lanes:?}");
        assert!(lanes.contains(&"long-context"));
        assert!(lanes.contains(&"priority"));
    }

    #[test]
    fn two_aliases_on_one_offering_do_not_take_turns() {
        // pass-index stage 2: a mis-bound second alias must show up as a
        // conflict, not as a price that changes on every run.
        let (_d, ix) = store();
        ix.bind_alias("prov_openrouter", "vendor/other-name", "ent_claude-opus-5").unwrap();
        let listings = vec![
            Listing {
                alias: "anthropic/claude-opus-5".into(),
                variant: String::new(),
                components: vec![("mtok_in".into(), 5_000_000)],
                payload: json!({}),
            },
            Listing {
                alias: "vendor/other-name".into(),
                variant: String::new(),
                components: vec![("mtok_in".into(), 9_000_000)],
                payload: json!({}),
            },
        ];
        let first =
            apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
                .unwrap();
        assert_eq!(first.conflicted, 1);
        let second =
            apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
                .unwrap();
        assert_eq!(second.appended, 0);
        let views = ix.offerings_of("ent_claude-opus-5").unwrap();
        assert_eq!(views[0].components[0].micros_per_unit, 5_000_000);
    }

    #[test]
    fn the_source_corrects_what_the_catalogue_assumed() {
        // pass-index: an entity minted as text->text is corrected by the
        // source that sells it, and the sentence is kept with its address.
        let (_d, ix) = store();
        let body = json!({"data": [{"id": "anthropic/claude-opus-5", "name": "Claude Opus 5",
            "context_length": 1000000,
            "description": "A frontier model for autonomous knowledge work.",
            "architecture": {"input_modalities": ["text","image","file"],
                             "output_modalities": ["text"]},
            "pricing": {"prompt": "0.000005", "completion": "0.000025"}}]});
        let listings = parse_openrouter(&body);
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
            .unwrap();
        assert_eq!(s.described, 1);
        let e = &ix.entities("model").unwrap()[0];
        assert_eq!(e.input_kind, "text + image + file");
        assert_eq!(e.output_kind, "text");
        assert!(e.attrs.contains("1000000"));
        let docs = ix.docs_of("ent_claude-opus-5").unwrap();
        assert!(docs.iter().any(|d| d["kind"] == "description"
            && d["text"].as_str().unwrap().starts_with("A frontier model")));
        assert!(docs.iter().any(|d| d["field"] == "input_kind"));
        // a second run states the same thing and corrects nothing
        let again = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-26", &listings)
            .unwrap();
        assert_eq!(again.described, 0);
    }

    #[test]
    fn a_lane_listing_does_not_restate_the_entity() {
        // pass-index: the fast lane's own context is not the model's.
        let (_d, ix) = store();
        ix.bind_alias("prov_openrouter", "anthropic/claude-opus-5-fast", "ent_claude-opus-5")
            .unwrap();
        let listings = vec![
            Listing { alias: "anthropic/claude-opus-5".into(), variant: String::new(),
                components: vec![("mtok_in".into(), 5_000_000)],
                payload: json!({"context_length": 1000000}) },
            Listing { alias: "anthropic/claude-opus-5-fast".into(), variant: String::new(),
                components: vec![("mtok_in".into(), 10_000_000)],
                payload: json!({"context_length": 200000}) },
        ];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        let e = &ix.entities("model").unwrap()[0];
        assert!(e.attrs.contains("1000000"), "the standard lane speaks: {}", e.attrs);
        let again = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-26", &listings)
            .unwrap();
        assert_eq!(again.described, 0, "a repeat run restates nothing");
    }

    #[test]
    fn silence_in_the_payload_changes_nothing() {
        // pass-index: a source that says nothing about modalities has not
        // said they are absent.
        let (_d, ix) = store();
        let listings = vec![Listing {
            alias: "anthropic/claude-opus-5".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 5_000_000)],
            payload: json!({"name": "Claude Opus 5"}),
        }];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        let e = &ix.entities("model").unwrap()[0];
        assert_eq!(e.input_kind, "text");
        assert_eq!(e.output_kind, "text");
    }

    #[test]
    fn a_twice_priced_dimension_is_dropped_not_guessed() {
        // pass-index stage 2: ambiguity never becomes a price row.
        let out = unambiguous(vec![
            ("mtok_in".into(), 100),
            ("mtok_in".into(), 200),
            ("mtok_out".into(), 300),
            ("mtok_out".into(), 300),
        ]);
        assert_eq!(out, vec![("mtok_out".into(), 300)]);
    }

    #[test]
    fn a_lane_alias_bound_to_the_base_entity_keeps_its_own_offering() {
        // pass-index stage 2: two lanes, two offerings, two stable prices —
        // the bug this replaced let the fast price overwrite the standard one
        // on every run.
        let (_d, ix) = store();
        ix.bind_alias("prov_openrouter", "anthropic/claude-opus-5-fast", "ent_claude-opus-5")
            .unwrap();
        let listings = vec![
            Listing {
                alias: "anthropic/claude-opus-5".into(),
                variant: String::new(),
                components: vec![("mtok_in".into(), 5_000_000)],
                payload: json!({}),
            },
            Listing {
                alias: "anthropic/claude-opus-5-fast".into(),
                variant: String::new(),
                components: vec![("mtok_in".into(), 10_000_000)],
                payload: json!({}),
            },
        ];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        let second =
            apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
                .unwrap();
        assert_eq!(second.appended, 0, "a repeat run must write nothing");
        let views = ix.offerings_of("ent_claude-opus-5").unwrap();
        assert_eq!(views.len(), 2);
        let fast = views.iter().find(|v| v.variant == "fast").unwrap();
        assert_eq!(fast.components[0].micros_per_unit, 10_000_000);
        let std_lane = views.iter().find(|v| v.variant.is_empty()).unwrap();
        assert_eq!(std_lane.components[0].micros_per_unit, 5_000_000);
    }

    #[test]
    fn a_preview_of_a_known_model_is_its_own_lane() {
        // pass-index: a source selling both the preview and the released
        // model prices two lanes of one model, not one price twice.
        let (_d, ix) = store();
        ix.bind_alias("prov_openrouter", "anthropic/claude-opus-5-preview", "ent_claude-opus-5")
            .unwrap();
        let listings = vec![
            Listing { alias: "anthropic/claude-opus-5".into(), variant: String::new(),
                components: vec![("mtok_in".into(), 5_000_000)], payload: json!({}) },
            Listing { alias: "anthropic/claude-opus-5-preview".into(), variant: String::new(),
                components: vec![("mtok_in".into(), 4_000_000)], payload: json!({}) },
        ];
        let s = apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings)
            .unwrap();
        assert_eq!(s.conflicted, 0);
        let views = ix.offerings_of("ent_claude-opus-5").unwrap();
        assert_eq!(views.len(), 2);
        assert!(views.iter().any(|v| v.variant == "preview"));
    }

    #[test]
    fn a_model_whose_name_ends_in_fast_is_not_a_lane() {
        // pass-index stage 2: the base must name something, or the suffix is
        // part of the model's own name.
        let (_d, ix) = store();
        ix.insert_entity(&Entity {
            id: "ent_morph-v3-fast".into(),
            register: "model".into(),
            name: "Morph V3 Fast".into(),
            input_kind: "text".into(),
            output_kind: "text".into(),
            attrs: "{}".into(),
            ..Default::default()
        })
        .unwrap();
        ix.bind_alias("prov_openrouter", "morph/morph-v3-fast", "ent_morph-v3-fast").unwrap();
        let listings = vec![Listing {
            alias: "morph/morph-v3-fast".into(),
            variant: String::new(),
            components: vec![("mtok_in".into(), 900_000)],
            payload: json!({}),
        }];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        let views = ix.offerings_of("ent_morph-v3-fast").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].variant, "");
    }

    #[test]
    fn a_source_priced_batch_lane_lands_as_variant() {
        // pass-index stage 2: listing.variant reaches the offering row.
        let (_d, ix) = store();
        let listings = vec![Listing {
            alias: "anthropic/claude-opus-5".into(),
            variant: "batch".into(),
            components: vec![("mtok_in".into(), 2_500_000)],
            payload: json!({}),
        }];
        apply(&ix, "prov_openrouter", "aggregator", "https://src", "2026-08-25", &listings).unwrap();
        let views = ix.offerings_of("ent_claude-opus-5").unwrap();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].variant, "batch");
    }

    #[test]
    fn today_utc_is_a_date() {
        let d = today_utc();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }
}

// ---------------------------------------------------------------------------
// The gateways that publish a flat rate card
// ---------------------------------------------------------------------------

/// Dollars per token, turned into micro-dollars per million.
pub const PER_TOKEN: f64 = 1e12;
/// Dollars per million, turned into micro-dollars per million.
pub const PER_MILLION: f64 = 1e6;

/// A figure a seller printed, read as money or not at all.
///
/// A string and a number both appear in these feeds, sometimes for the same
/// field on the same day, so both are accepted. A nought is refused: here it
/// means a rate that rounded away or a field the seller left empty, never a
/// declaration that something is free — that arrives on its own lane.
fn money(v: &Value, scale: f64) -> Option<i64> {
    let f = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.parse().ok()?,
        _ => return None,
    };
    (f > 0.0).then(|| (f * scale).round() as i64)
}

/// The common shape: one dict of rates per model, in one unit.
///
/// Half a dozen gateways publish the OpenAI models list with their own names
/// for the fields, so the difference between them is a table rather than a
/// parser. What the source calls a model stays its own string — binding is
/// the operator's job.
pub fn parse_flat(body: &Value, scale: f64, keys: &[(&str, &str)]) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let p = &m["pricing"];
        if !p.is_object() {
            continue;
        }
        let mut components = Vec::new();
        for (theirs, ours) in keys {
            if let Some(micros) = money(&p[*theirs], scale) {
                components.push((ours.to_string(), micros));
            }
        }
        if components.is_empty() {
            continue;
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: m.clone(),
        });
    }
    out
}

/// PPIO nests each rate and prints it twice, once scaled and once as a
/// decimal string. The string is the one that says what it means.
pub fn parse_ppinfra(body: &Value) -> Vec<Listing> {
    let mut out = Vec::new();
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(alias) = m["id"].as_str() else { continue };
        let mut components = Vec::new();
        for (theirs, ours) in [("prompt", "mtok_in"), ("completion", "mtok_out")] {
            if let Some(micros) =
                money(&m["pricing"][theirs]["price_per_m_decimal"], PER_MILLION)
            {
                components.push((ours.to_string(), micros));
            }
        }
        if components.is_empty() {
            continue;
        }
        out.push(Listing {
            alias: alias.to_string(),
            variant: String::new(),
            components,
            payload: m.clone(),
        });
    }
    out
}

#[cfg(test)]
mod gateway_tests {
    use super::*;
    use serde_json::json;

    /// The scale is the whole difference between a plausible rate and one out
    /// by a factor of a million, so both units are exercised.
    #[test]
    fn a_flat_card_reads_in_the_unit_the_seller_uses() {
        let body = json!({"data": [
            {"id": "a/b", "pricing": {"input": 1.5, "output": "3"}},
            {"id": "c/d", "pricing": {"input": 0, "output": null}},
            {"id": "e/f"},
        ]});
        let got = parse_flat(&body, PER_MILLION, &[("input", "mtok_in"), ("output", "mtok_out")]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].alias, "a/b");
        assert_eq!(
            got[0].components,
            vec![("mtok_in".into(), 1_500_000), ("mtok_out".into(), 3_000_000)]
        );

        let per_token = json!({"data": [{"id": "x", "pricing": {"prompt": "0.0000015"}}]});
        let got = parse_flat(&per_token, PER_TOKEN, &[("prompt", "mtok_in")]);
        assert_eq!(got[0].components, vec![("mtok_in".into(), 1_500_000)]);
    }

    #[test]
    fn ppio_is_read_from_the_string_that_says_what_it_means() {
        let body = json!({"data": [{"id": "m", "pricing": {
            "prompt": {"price_per_m": 200000, "price_per_m_decimal": "0.2"},
            "completion": {"price_per_m_decimal": "0.8"}}}]});
        let got = parse_ppinfra(&body);
        assert_eq!(
            got[0].components,
            vec![("mtok_in".into(), 200_000), ("mtok_out".into(), 800_000)]
        );
    }
}

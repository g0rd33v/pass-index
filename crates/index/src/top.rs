//! The picks: four answers per niche, each one sort with one constraint.
//!
//! Everywhere else the catalogue reports; here it recommends, and a
//! recommendation has to survive being argued with. So there is no weighted
//! score anywhere in this file. Each pick is a single ordering with a single
//! stated restriction, and the page prints both:
//!
//!   value    — the most capable of those priced at or below the median
//!   frontier — the most capable, price no object
//!   open     — the most capable whose weights you may have
//!   cheapest — the least expensive of those above the median capability
//!
//! Value and cheapest are duals: the best of the cheap half, the cheapest of
//! the good half. The floor on `cheapest` is not decoration. Without it the
//! answer for embeddings was the model ranked last of thirty-nine, and for
//! coding a model in the fourteenth percentile — cheap because nobody wants
//! it. A recommendation that is reliably the worst thing in the category is
//! worse than no recommendation.
//!
//! Three rules earned by being broken:
//!
//! - **A niche is a set of boards, not a tag.** Standing on SWE-bench is what
//!   makes a model a coder. Read from tags instead and "best coder" comes out
//!   as whoever leads LMArena Text, which is how the first draft answered.
//! - **One metric per board.** A board publishes several cuts, and the small
//!   ones are the flattering ones. Taking a model's best across all of them
//!   let a model ranked 4th of 9 outrank one ranked 11th of 114. The headline
//!   metric is the widest one, and it is the only one counted.
//! - **Only what you can buy today.** A pick with no seller and no price is
//!   not an answer to "what should I use". The first draft crowned a TTS
//!   model nobody sells.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::params;
use serde_json::{json, Value};

use crate::{address_slug, Index};

/// What a buyer is trying to do, and the boards that measure it.
///
/// The unit is the one the whole niche is compared in; a model priced in
/// anything else can still be named best, but cannot be called cheapest,
/// because the comparison would be between different things.
pub struct Niche {
    pub key: &'static str,
    pub title: &'static str,
    pub question: &'static str,
    pub unit: &'static str,
    pub boards: &'static [&'static str],
    /// Which block of the page this belongs to. The families read in the
    /// order a person works through them: what a model does with words,
    /// then with pictures, then with a voice, then the plumbing, and last
    /// the two halves of the market.
    pub family: &'static str,
    /// The tag a thing must carry to be picked here. A niche's boards are
    /// chosen for the work, but a board can rank things that do not do that
    /// work — MTEB's reranking split ranks embedding models — and the best
    /// reranker on the page was an embedding model for that reason.
    pub task: Option<&'static str>,
    /// Half of the market rather than a kind of work: "open" or
    /// "proprietary". A niche cut this way drops its open-source pick,
    /// because inside it the answer is either every row or none of them.
    pub half: Option<&'static str>,
}

pub const NICHES: &[Niche] = &[
    Niche { key: "chat", family: "Text", title: "General",
        question: "which model to reach for when the job is not a specialised one",
        unit: "mtok", boards: &["lmarena_text", "lmarena_search",
            "epoch_simpleqa_verified", "epoch_lech_mazur_writing", "epoch_wino_grande", "epoch_piqa", "epoch_bool_q", "epoch_arc_ai2",
            "epoch_science_qa", "epoch_open_book_qa", "epoch_adversarial_nli", "epoch_lambada", "epoch_common_sense_qa_2"],
        // The general niche is the one for work that is not specialised, so
        // it takes no tag: the models a reader reaches for here are filed
        // under reasoning and code, and asking for the tag "chat" excluded
        // every one of them — twenty-seven measured, one you could buy.
        task: None, half: None },
    Niche { key: "code", family: "Text", title: "Coding",
        question: "which model should write and fix code",
        unit: "mtok", boards: &["swebench_pro_public", "swebench_verified",
            "swebench_verified_all_agents", "swe_rebench", "livecodebench", "aider_polyglot",
            "terminal_bench_2_1", "terminal_bench_2_0", "swebench_multimodal", "lmarena_webdev",
            "epoch_scicode", "epoch_cursorbench", "epoch_deepswe", "epoch_frontiercode", "epoch_swe_bench_verified", "epoch_terminalbench", "epoch_webdev_arena", "epoch_algotune", "epoch_surface_evolver_bench"], task: Some("code"), half: None },
    Niche { key: "agents", family: "Text", title: "Agents",
        question: "which model can call tools and finish a job",
        unit: "mtok", boards: &["bfcl_v4", "tau3_banking", "tau2_bench_overall", "agentbench",
            "lmarena_agent", "vending_bench_2", "osworld_verified", "webarena", "gaia",
            "epoch_apex_agents", "epoch_deepresearchbench", "epoch_the_agent_company", "epoch_os_world", "epoch_osworld_2", "epoch_blueprint_bench_2", "epoch_forecastbench", "epoch_metr_time_horizons", "epoch_rli", "epoch_btf3",
            "epoch_vending_bench_2"], task: None, half: None },
    Niche { key: "reasoning", family: "Text", title: "Reasoning",
        question: "which model can do the mathematics and the science",
        unit: "mtok", boards: &["epoch_frontiermath", "epoch_hle", "hle", "gpqa_diamond",
            "mmlu_pro", "aime_2026", "aime_2025", "epoch_otis_mock_aime",
            "arc_agi_1", "arc_agi_2", "arc_agi_3", "aa_intelligence",
            "epoch_capabilities_index", "epoch_weirdml", "epoch_critpt", "epoch_chess_puzzles", "epoch_math_level_5", "epoch_arc_agi", "epoch_arc_agi_2", "epoch_simplebench", "epoch_proofbench", "epoch_frontiermath_tier_4", "epoch_mystery_game_puzzles", "epoch_enigma_eval", "epoch_gbaeval",
            "arc_agi_1_public", "arc_agi_2_public"], task: Some("reasoning"), half: None },
    Niche { key: "video", family: "Visual", title: "Video generation",
        question: "which model makes the clip",
        unit: "second", boards: &["aa_text_to_video"], task: Some("video"), half: None },
    Niche { key: "image", family: "Visual", title: "Image generation",
        question: "which model draws the picture",
        unit: "image", boards: &["aa_text_to_image"], task: Some("image"), half: None },
    Niche { key: "vision", family: "Visual", title: "Image recognition",
        question: "which model understands a picture or a document",
        unit: "mtok", boards: &["lmarena_vision",
            "epoch_spatialviz_bench"], task: None, half: None },
    Niche { key: "speech", family: "Voice", title: "Speech generation",
        question: "which model reads your text out loud",
        unit: "character", boards: &["tts_arena_v2"], task: Some("speak"), half: None },
    Niche { key: "transcription", family: "Voice", title: "Speech recognition",
        question: "which model turns speech into text",
        unit: "minute", boards: &["open_asr_en_short"], task: Some("transcribe"), half: None },
    Niche { key: "voice", family: "Voice", title: "Live voice",
        question: "which model can take a call and handle it",
        unit: "mtok", boards: &["tau3_voice"], task: None, half: None },
    Niche { key: "embedding", family: "Retrieval", title: "Embeddings",
        question: "which model to build a search index on",
        unit: "mtok_in", boards: &["mteb_multi", "mteb_eng"], task: Some("embedding"), half: None },
    Niche { key: "rerank", family: "Retrieval", title: "Reranking",
        question: "which model puts the right answer at the top",
        unit: "mtok_in", boards: &["mteb_eng_rerank"], task: Some("rerank"), half: None },
    Niche { key: "open-weights", family: "The market in halves", title: "Open weights",
        question: "which model to run yourself, or buy from whoever you like",
        unit: "mtok", boards: &[], task: None, half: Some("open") },
    Niche { key: "proprietary", family: "The market in halves", title: "Proprietary",
        question: "which model behind an API is worth the money",
        unit: "mtok", boards: &[], task: None, half: Some("proprietary") },
];

/// Licences that let you take the weights and sell what they produce. The
/// same set the licence lists use, kept as a slice because "best open source"
/// must mean exactly one thing across the whole catalogue.
const OPEN: &[&str] = &["apache-2.0", "mit", "cc-by-4.0", "bsd-3-clause", "openmdw-1.1"];

/// Whether the weights are yours to have.
///
/// Two facts answer this and they are not the same fact. A licence read off a
/// model card names the terms; models.dev's field says only that the weights
/// are published, which is enough for this question and not enough to print a
/// licence name. Either answers yes, and a model with neither is unread rather
/// than closed — silence has never meant proprietary here.
fn is_open(
    e: &String,
    licences: &std::collections::HashMap<String, String>,
    published: &std::collections::HashSet<String>,
) -> bool {
    licences.get(e).map(|l| OPEN.contains(&l.as_str())).unwrap_or(false) || published.contains(e)
}

/// What each block is for, in the order it is read. A family says the thing
/// its groups have in common, so the reader can skip a whole block rather
/// than read fourteen headings to find the one they came for.
pub const FAMILIES: &[(&str, &str)] = &[
    ("Text", "Work done in words: the everyday model, the one that writes code, \
              the one that can be sent off to finish a job, and the one that does \
              the mathematics."),
    ("Visual", "Making a picture or a clip, and reading one."),
    ("Voice", "Speaking, listening, and holding a conversation aloud — three \
               different markets that are easy to mistake for one."),
    ("Retrieval", "The plumbing under a search box: turning text into vectors, \
                   and putting the right answer at the top."),
    ("The market in halves", "The same question asked of each side of the \
                              industry. Open weights are measured far less often \
                              than closed ones, so that block is the thinnest on \
                              the page, and it is last for that reason."),
];

pub fn niche(key: &str) -> Option<&'static Niche> {
    NICHES.iter().find(|n| n.key == key)
}

/// How the unit reads beside a figure, and what a price in it is quoted per.
pub fn unit_words(unit: &str) -> &'static str {
    crate::unit_phrase(unit)
}

struct Standing {
    /// Mean percentile across the niche's boards this model appears on.
    /// The mean, not the best: a model that leads one small board and
    /// trails four large ones has not won the niche.
    cap: f64,
    boards: usize,
    /// The single board it does best on, for the row to cite.
    best: (String, usize, usize),
}

impl Index {
    /// Every niche with its four picks, for the overview page.
    pub fn top_index(&self) -> Result<Vec<Value>> {
        NICHES
            .iter()
            .map(|n| self.top_page(n.key).map(|v| v.unwrap_or(json!({}))))
            .filter(|v| v.as_ref().map(|v| !v["picks"].as_array().map_or(true, |a| a.is_empty())).unwrap_or(true))
            .collect()
    }

    /// One niche: who wins it, on what evidence, at what price.
    /// Total published parameter count as billions, per entity, for the models
    /// that state one. Sparse by design: the count is read from the model card
    /// (or a size token in the name), never inferred — a model without one is
    /// absent here, not zero.
    fn param_billions(&self) -> Result<HashMap<String, f64>> {
        let mut q = self.conn.prepare(
            "SELECT id, json_extract(attrs,'$.params') FROM entities \
             WHERE json_extract(attrs,'$.params') IS NOT NULL",
        )?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut m = HashMap::new();
        for row in rows {
            let (id, p) = row?;
            if p > 0 {
                m.insert(id, p as f64 / 1e9);
            }
        }
        Ok(m)
    }

    /// The per-model niche standing: the mean percentile across the niche's
    /// boards. This is the SAME capability the `top_page` picks are chosen by;
    /// its computation mirrors the block in `top_page` below (top.rs) and must
    /// stay in step with it — the "Bang for a buck" page ranks by exactly this,
    /// so a divergent definition here would be a second, private notion of
    /// "capable". Returns the standings, the boards actually used, and the
    /// boards kept only for want of a better one.
    fn niche_stands(
        &self,
        n: &Niche,
    ) -> Result<(HashMap<String, Standing>, Vec<(String, String)>, Vec<String>)> {
        let mut widest: HashMap<String, (String, usize)> = HashMap::new();
        {
            let mut q = self.conn.prepare(
                "SELECT suite, metric, COUNT(DISTINCT entity_id) FROM benchmarks GROUP BY 1,2",
            )?;
            let rows = q.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize))
            })?;
            for row in rows {
                let (suite, metric, count) = row?;
                let e = widest.entry(suite).or_insert((metric.clone(), 0));
                if count > e.1 {
                    *e = (metric, count);
                }
            }
        }
        let mut sums: HashMap<String, (f64, usize)> = HashMap::new();
        let mut best: HashMap<String, (f64, String, usize, usize)> = HashMap::new();
        let mut boards_used: Vec<(String, String)> = Vec::new();
        let mut crowded: Vec<String> = Vec::new();
        let own: Vec<&str>;
        let boards: &[&str] = if n.half.is_some() {
            let mut v: Vec<&str> = NICHES
                .iter()
                .filter(|m| m.half.is_none() && m.unit.starts_with("mtok"))
                .flat_map(|m| m.boards.iter().copied())
                .collect();
            v.sort_unstable();
            v.dedup();
            own = v;
            &own
        } else {
            n.boards
        };
        for suite in boards {
            let Some((metric, _)) = widest.get(*suite) else { continue };
            let (name, lower): (String, i64) = match self.conn.query_row(
                "SELECT name, COALESCE(lower_is_better,0) FROM suites WHERE id=?1",
                params![suite],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut q = self.conn.prepare(
                "SELECT b.entity_id, b.value, b.rank, b.out_of FROM benchmarks b \
                  WHERE b.suite=?1 AND b.metric=?2 \
                    AND b.id=(SELECT MAX(id) FROM benchmarks x \
                               WHERE x.entity_id=b.entity_id AND x.suite=b.suite AND x.metric=b.metric)",
            )?;
            let mut rows: Vec<(String, f64, Option<i64>, Option<i64>)> = q
                .query_map(params![suite, metric], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
            if rows.len() < 8 {
                continue;
            }
            let mut sorted: Vec<f64> = rows.iter().map(|r| r.1).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let (lowest, highest) = (sorted[0], sorted[sorted.len() - 1]);
            let range = highest - lowest;
            let near_top = sorted[sorted.len() - 1 - (sorted.len() / 10).max(1)];
            let separates = range > 0.0 && (highest - near_top) / range >= 0.05;
            if !separates && boards.len() > 1 {
                continue;
            }
            if !separates {
                crowded.push(name.clone());
            }
            if lower == 1 {
                rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            } else {
                rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            }
            boards_used.push((suite.to_string(), name.clone()));
            let short = name
                .split(" (").next().unwrap_or(&name)
                .split(" — ").next().unwrap_or(&name)
                .trim().to_string();
            let ours = rows.len();
            for (i, (eid, _, rank, out_of)) in rows.iter().enumerate() {
                let (place, field) = match (rank, out_of) {
                    (Some(r), Some(o)) if *o > 1 && *r >= 1 => (*r as usize, *o as usize),
                    _ => (i + 1, ours),
                };
                if field < 2 {
                    continue;
                }
                let pct = 1.0 - (place - 1) as f64 / (field - 1) as f64;
                let e = sums.entry(eid.clone()).or_insert((0.0, 0));
                e.0 += pct;
                e.1 += 1;
                let b = best.entry(eid.clone()).or_insert((-1.0, String::new(), 0, 0));
                if pct > b.0 {
                    *b = (pct, short.clone(), place, field);
                }
            }
        }
        let stands: HashMap<String, Standing> = sums
            .into_iter()
            .map(|(eid, (total, n))| {
                let b = best.get(&eid).cloned().unwrap_or((0.0, String::new(), 0, 0));
                (eid, Standing { cap: total / n as f64, boards: n, best: (b.1, b.2, b.3) })
            })
            .collect();
        Ok((stands, boards_used, crowded))
    }

    /// "Bang for a buck": for each of the three categories — general reasoning,
    /// coding, agentic — the three most capable models whose OUTPUT sells for
    /// under $1 per 1M tokens, and the largest known parameter count (billions)
    /// available under that dollar as the category header. A model with no
    /// published parameter count still counts among the three (it is not a
    /// condition); it only cannot set the header maximum.
    pub fn bang(&self) -> Result<Value> {
        // Eugene's categories and their order, mapped to the niche keys. The
        // labels are his, verbatim.
        const CATS: &[(&str, &str)] = &[
            ("reasoning", "General reasoning"),
            ("code", "Coding"),
            ("agents", "Agentic"),
        ];
        const UNDER: f64 = 1.0; // dollars per 1M output tokens

        let params = self.param_billions()?;
        let pair = self.token_prices()?; // entity -> (in, out) dollars per 1M
        let sellers = self.seller_counts()?;
        let makers = self.makers()?;
        let tasks = self.tasks_of()?;
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();

        let mut blocks = Vec::new();
        for (key, label) in CATS {
            let Some(n) = niche(key) else { continue };
            let (stands, _boards, _crowded) = self.niche_stands(n)?;

            // Eligible = a model this category measures, that somebody sells, at
            // an output price under the dollar. Parameters are not consulted here.
            let mut elig: Vec<(String, f64, f64, Option<f64>)> = stands
                .iter()
                .filter_map(|(eid, s)| {
                    let out = pair.get(eid).map(|p| p.1)?;
                    if out >= UNDER {
                        return None;
                    }
                    if sellers.get(eid).copied().unwrap_or(0) == 0 || !addr.contains_key(eid) {
                        return None;
                    }
                    if let Some(t) = n.task {
                        if !tasks.get(eid).map(|ts| ts.iter().any(|x| x == t)).unwrap_or(false) {
                            return None;
                        }
                    }
                    Some((eid.clone(), s.cap, out, params.get(eid).copied()))
                })
                .collect();

            // Header: the biggest brain a dollar buys in this category — the max
            // of the known counts among the eligible, or null when none states one.
            let max_params = elig
                .iter()
                .filter_map(|(_, _, _, p)| *p)
                .fold(None::<f64>, |m, p| Some(m.map_or(p, |x: f64| x.max(p))));

            // The three most capable of them.
            elig.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let picks: Vec<Value> = elig
                .iter()
                .take(3)
                .map(|(eid, cap, out, p)| {
                    let s = &stands[eid];
                    let (name, href) = addr.get(eid).cloned().unwrap_or_default();
                    json!({
                        "name": name, "href": href,
                        "maker": makers.get(eid).cloned().unwrap_or_default(),
                        "capability": (cap * 100.0).round(),
                        "out": out,
                        "params_b": p,
                        "board": s.best.0, "rank": s.best.1, "field": s.best.2,
                    })
                })
                .collect();

            blocks.push(json!({
                "key": key, "title": label, "under": UNDER,
                "max_params_b": max_params, "eligible": elig.len(), "picks": picks,
            }));
        }
        Ok(json!({ "kind": "bang", "under": UNDER, "blocks": blocks }))
    }

    pub fn top_page(&self, key: &str) -> Result<Option<Value>> {
        let Some(n) = niche(key) else { return Ok(None) };

        // A board publishes several metrics; the widest is the headline, and
        // the narrow ones are where a model goes shopping for a better rank.
        let mut widest: HashMap<String, (String, usize)> = HashMap::new();
        {
            let mut q = self.conn.prepare(
                "SELECT suite, metric, COUNT(DISTINCT entity_id) FROM benchmarks GROUP BY 1,2",
            )?;
            let rows = q.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as usize))
            })?;
            for row in rows {
                let (suite, metric, count) = row?;
                let e = widest.entry(suite).or_insert((metric.clone(), 0));
                if count > e.1 {
                    *e = (metric, count);
                }
            }
        }

        // Percentile within each board of the niche, then the mean.
        let mut sums: HashMap<String, (f64, usize)> = HashMap::new();
        let mut best: HashMap<String, (f64, String, usize, usize)> = HashMap::new();
        let mut boards_used: Vec<(String, String)> = Vec::new();
        // Boards kept for want of a better one, and named on the page so the
        // pick is read for what it is.
        let mut crowded: Vec<String> = Vec::new();
        // A half of the market is not a kind of work, so it has no boards of
        // its own: it is ranked across every board that measures work billed
        // by the token. Gathering that here rather than keeping a second list
        // means it cannot fall behind the niches — which it had, by 66 boards.
        let own: Vec<&str>;
        let boards: &[&str] = if n.half.is_some() {
            let mut v: Vec<&str> = NICHES
                .iter()
                .filter(|m| m.half.is_none() && m.unit.starts_with("mtok"))
                .flat_map(|m| m.boards.iter().copied())
                .collect();
            v.sort_unstable();
            v.dedup();
            own = v;
            &own
        } else {
            n.boards
        };
        for suite in boards {
            let Some((metric, _)) = widest.get(*suite) else { continue };
            let (name, lower): (String, i64) = match self.conn.query_row(
                "SELECT name, COALESCE(lower_is_better,0) FROM suites WHERE id=?1",
                params![suite],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let mut q = self.conn.prepare(
                "SELECT b.entity_id, b.value, b.rank, b.out_of FROM benchmarks b \
                  WHERE b.suite=?1 AND b.metric=?2 \
                    AND b.id=(SELECT MAX(id) FROM benchmarks x \
                               WHERE x.entity_id=b.entity_id AND x.suite=b.suite AND x.metric=b.metric)",
            )?;
            let mut rows: Vec<(String, f64, Option<i64>, Option<i64>)> = q
                .query_map(params![suite, metric], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<std::result::Result<_, _>>()?;
            // A field of a handful ranks nobody: being third of four is not
            // evidence of anything, and a percentile computed on it is noise.
            if rows.len() < 8 {
                continue;
            }
            // Nor does a board everybody has already topped. Winogrande's
            // fourteen entrants sit within two per cent of each other, and
            // ARC-AI2's eight are identical to three decimal places — so
            // whoever happens to be first there is first by nothing, and a
            // 2024 model was winning "the best there is" on that alone.
            //
            // Such a board is set aside — unless it is the only one the niche
            // has. Weak evidence is not no evidence, and dropping the last
            // board deletes the whole niche from the page, which is a worse
            // answer than a hedged one.
            let mut sorted: Vec<f64> = rows.iter().map(|r| r.1).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let (lowest, highest) = (sorted[0], sorted[sorted.len() - 1]);
            let range = highest - lowest;
            let near_top = sorted[sorted.len() - 1 - (sorted.len() / 10).max(1)];
            let separates = range > 0.0 && (highest - near_top) / range >= 0.05;
            if !separates && boards.len() > 1 {
                continue;
            }
            if !separates {
                crowded.push(name.clone());
            }
            if lower == 1 {
                rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            } else {
                rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            }
            boards_used.push((suite.to_string(), name.clone()));
            // A board's own title carries its footnotes — "(Bash Only toggle
            // off — all agents)", "— Current leaderboard". The name is what
            // the row cites; the board's page keeps the rest.
            let short = name
                .split(" (").next().unwrap_or(&name)
                .split(" — ").next().unwrap_or(&name)
                .trim().to_string();
            // The board publishes where a model came and how many it beat.
            // Ranking inside our own matched slice instead said "4th of 22"
            // for a model the board itself places 8th of 394 — a different
            // claim, and the wrong one. The slice only decides who is in the
            // pool; the standing is the board's own.
            let ours = rows.len();
            for (i, (eid, _, rank, out_of)) in rows.iter().enumerate() {
                let (place, field) = match (rank, out_of) {
                    (Some(r), Some(o)) if *o > 1 && *r >= 1 => (*r as usize, *o as usize),
                    _ => (i + 1, ours),
                };
                if field < 2 {
                    continue;
                }
                let pct = 1.0 - (place - 1) as f64 / (field - 1) as f64;
                let e = sums.entry(eid.clone()).or_insert((0.0, 0));
                e.0 += pct;
                e.1 += 1;
                let b = best.entry(eid.clone()).or_insert((-1.0, String::new(), 0, 0));
                if pct > b.0 {
                    *b = (pct, short.clone(), place, field);
                }
            }
        }
        if sums.is_empty() {
            return Ok(None);
        }
        let stands: HashMap<String, Standing> = sums
            .into_iter()
            .map(|(eid, (total, n))| {
                let b = best.get(&eid).cloned().unwrap_or((0.0, String::new(), 0, 0));
                (eid, Standing { cap: total / n as f64, boards: n, best: (b.1, b.2, b.3) })
            })
            .collect();

        let cost = self.costs(n.unit)?;
        let pair = if n.unit == "mtok" { self.token_prices()? } else { HashMap::new() };
        let sellers = self.seller_counts()?;
        let licences = self.licences()?;
        let tasks = self.tasks_of()?;
        let published = self.published_weights()?;
        let makers = self.makers()?;
        let addr: HashMap<String, (String, String)> = self
            .entity_addresses()?
            .into_iter()
            .map(|(id, name, head, tail)| (id, (name, format!("/index/{head}/{tail}"))))
            .collect();
        // Where a niche is measured many ways, one result is not a verdict.
        // Topping AgentBench alone made a 7B model the best agent in the
        // catalogue while the models that were asked seven different
        // questions ranked below it. Two boards is the cheapest possible
        // corroboration; a niche with three boards or fewer cannot ask for
        // it and does not.
        let corroborate = boards_used.len() >= 4;

        // Only a model somebody sells, at a price in this niche's unit.
        // What the niche is about. A board can rank things that do not do the
        // work it is named for, and without this the best reranker on the
        // page was an embedding model.
        let does_the_work = |e: &String| -> bool {
            match n.task {
                None => true,
                Some(t) => tasks.get(e).map(|ts| ts.iter().any(|x| x == t)).unwrap_or(false),
            }
        };
        let in_half = |e: &String| -> bool {
            match n.half {
                None => true,
                Some("open") => is_open(e, &licences, &published),
                Some(_) => licences.get(e).map(|l| l == "proprietary").unwrap_or(false),
            }
        };
        // Counted after the half is applied, not before. Open weights and
        // Proprietary each reported "389 measured, 307 for sale" — the same
        // pair, because the counts were of the whole niche and the list
        // underneath was of half of it.
        let measured: usize = stands.keys().filter(|e| in_half(e) && does_the_work(e)).count();
        let buyable: usize = stands
            .keys()
            .filter(|e| sellers.get(*e).copied().unwrap_or(0) > 0 && cost.contains_key(*e)
                        && addr.contains_key(*e) && in_half(e) && does_the_work(e))
            .count();
        let pool: Vec<&String> = stands
            .keys()
            .filter(|e| sellers.get(*e).copied().unwrap_or(0) > 0 && cost.contains_key(*e)
                        && addr.contains_key(*e)
                        && (!corroborate || stands[*e].boards >= 2)
                        && in_half(e) && does_the_work(e))
            .collect();
        if pool.len() < 4 {
            return Ok(Some(json!({
                "kind": "top", "key": n.key, "title": n.title, "question": n.question,
                "family": n.family,
                "measured": measured, "buyable": pool.len(), "eligible": pool.len(),
                "picks": [],
                "boards": boards_used.iter().map(|(id, nm)|
                    json!({"id": id, "name": nm, "href": format!("/index/board/{}", address_slug(id))}))
                    .collect::<Vec<_>>(),
            })));
        }

        let mut by_cost: Vec<f64> = pool.iter().map(|e| cost[*e]).collect();
        by_cost.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let price_median = by_cost[by_cost.len() / 2];
        let mut by_cap: Vec<f64> = pool.iter().map(|e| stands[*e].cap).collect();
        by_cap.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let cap_median = by_cap[by_cap.len() / 2];

        let opens: Vec<&&String> = pool
            .iter()
            .filter(|e| is_open(**e, &licences, &published))
            .collect();
        // How much of this field we have actually read a licence for. Silence
        // is not proprietary: an unread model card leaves a model neither
        // open nor closed, and a "best open source" drawn from a third of the
        // field is a guess wearing a fact's clothes.
        let known = pool
            .iter()
            .filter(|e| licences.contains_key(**e) || published.contains(**e))
            .count();
        let read_share = known as f64 / pool.len() as f64;

        let most_capable = |set: &[&String]| -> Option<String> {
            set.iter()
                .max_by(|a, b| stands[**a].cap.partial_cmp(&stands[**b].cap).unwrap())
                .map(|s| (*s).clone())
        };
        let least_costly = |set: &[&String]| -> Option<String> {
            set.iter()
                .min_by(|a, b| cost[**a].partial_cmp(&cost[**b]).unwrap())
                .map(|s| (*s).clone())
        };

        let cheap_half: Vec<&String> =
            pool.iter().filter(|e| cost[**e] <= price_median).cloned().collect();
        let good_half: Vec<&String> =
            pool.iter().filter(|e| stands[**e].cap >= cap_median).cloned().collect();
        let open_pool: Vec<&String> = opens.iter().map(|e| **e).collect();

        let want_open = n.half.is_none();
        let chosen = [
            ("value", "if you want the most for your money",
             format!("the most capable at or below ${:.4} {}", price_median, unit_words(n.unit)),
             most_capable(&cheap_half)),
            ("frontier", "if you want the best there is",
             "the most capable anyone sells, whatever it costs".to_string(),
             most_capable(&pool)),
            ("open", "if the weights have to be yours",
             format!("the most capable you may download and sell, of the {known} \
                      of {} whose weights we have an answer about", pool.len()),
             most_capable(&open_pool)
                 .filter(|_| want_open)
                 .filter(|e| stands[e].cap >= cap_median)),
            ("cheapest", "if you only need it to work",
             "the least expensive that still beats half the field".to_string(),
             least_costly(&good_half)),
        ];

        let picks: Vec<Value> = chosen
            .iter()
            .filter_map(|(cell, lead, rule, who)| {
                let eid = who.as_ref()?;
                let s = &stands[eid];
                let (name, href) = addr.get(eid)?;
                Some(json!({
                    "cell": cell, "lead": lead, "rule": rule,
                    "entity": eid, "name": name, "href": href,
                    "maker": makers.get(eid).cloned().unwrap_or_default(),
                    "cost": cost[eid], "unit": n.unit,
                    "in": pair.get(eid).map(|p| p.0), "out": pair.get(eid).map(|p| p.1),
                    "capability": (s.cap * 100.0).round(),
                    "board": s.best.0, "rank": s.best.1, "field": s.best.2, "boards": s.boards,
                    "sellers": sellers.get(eid).copied().unwrap_or(0),
                    "licence": licences.get(eid).cloned().unwrap_or_default(),
                }))
            })
            .collect();

        Ok(Some(json!({
            "kind": "top", "key": n.key, "title": n.title, "question": n.question,
            "family": n.family, "unit": n.unit, "unit_words": unit_words(n.unit),
            "measured": measured, "buyable": buyable, "eligible": pool.len(),
            "open": opens.len(), "half": n.half,
            "median_price": price_median, "corroborated": corroborate,
            "licence_read": known, "licence_share": (read_share * 100.0).round(),
            "picks": picks,
            "boards": boards_used.iter().map(|(id, nm)|
                json!({"id": id, "name": nm, "href": format!("/index/board/{}", address_slug(id))}))
                .collect::<Vec<_>>(),
            "crowded": crowded,
        })))
    }

    /// The cheapest standard-lane price per entity, in one unit.
    ///
    /// Standard lane only: a batch or flex rate is real but it is not what
    /// you pay for an answer now, and a pick quoted at the batch rate sends
    /// a reader to budget against a price that does not exist for them.
    fn costs(&self, unit: &str) -> Result<HashMap<String, f64>> {
        let mut out = HashMap::new();
        if unit == "mtok_in" {
            let mut q = self.conn.prepare(
                "SELECT o.entity_id, MIN(p.micros_per_unit) FROM offerings o \
                   JOIN current_prices p ON p.offering_id=o.id \
                  WHERE p.dimension='mtok_in' AND p.micros_per_unit > 0 \
                    AND o.status='live' \
                    AND (o.variant IS NULL OR o.variant='') GROUP BY 1",
            )?;
            let rows = q.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
            })?;
            for row in rows {
                let (eid, mu) = row?;
                out.insert(eid, mu / 1e6);
            }
            return Ok(out);
        }
        if unit == "mtok" {
            let mut q = self.conn.prepare(
                "SELECT o.entity_id, \
                        MIN(CASE WHEN p.dimension='mtok_in'  THEN p.micros_per_unit END), \
                        MIN(CASE WHEN p.dimension='mtok_out' THEN p.micros_per_unit END) \
                   FROM offerings o JOIN current_prices p ON p.offering_id=o.id \
                  WHERE (o.variant IS NULL OR o.variant='') AND p.micros_per_unit > 0 \
                    AND o.status='live' \
                  GROUP BY 1",
            )?;
            let rows = q.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<f64>>(2)?))
            })?;
            for row in rows {
                let (eid, i, o) = row?;
                // A chat costs roughly three tokens out for every one in;
                // quoting input alone flatters whoever charges for output.
                if let (Some(i), Some(o)) = (i, o) {
                    out.insert(eid, (i + 3.0 * o) / 4.0 / 1e6);
                }
            }
            return Ok(out);
        }
        // An aggregator's per-image figure is what it charges to *read* an
        // image, not to draw one — two micros against the maker's own
        // thirteen cents. Only the seller of the model itself is asked.
        let extra = if unit == "image" { " AND pr.kind <> 'aggregator'" } else { "" };
        let scale = if unit == "character" { 1e6 } else { 1.0 };
        let sql = format!(
            "SELECT o.entity_id, MIN(p.micros_per_unit) FROM offerings o \
               JOIN current_prices p ON p.offering_id=o.id \
               JOIN providers pr ON pr.id=o.provider_id \
              WHERE p.dimension=?1 AND p.micros_per_unit > 0 \
                AND o.status='live' \
                AND (o.variant IS NULL OR o.variant=''){extra} GROUP BY 1"
        );
        let mut q = self.conn.prepare(&sql)?;
        let rows = q.query_map(params![unit], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        for row in rows {
            let (eid, mu) = row?;
            out.insert(eid, mu / 1e6 * scale);
        }
        Ok(out)
    }

    /// What a model charges to read and to write, kept apart. The blended
    /// figure orders the list; nobody is ever billed it, so nobody is shown
    /// it as a price.
    fn token_prices(&self) -> Result<HashMap<String, (f64, f64)>> {
        let mut q = self.conn.prepare(
            "SELECT o.entity_id, \
                    MIN(CASE WHEN p.dimension='mtok_in'  THEN p.micros_per_unit END), \
                    MIN(CASE WHEN p.dimension='mtok_out' THEN p.micros_per_unit END) \
               FROM offerings o JOIN current_prices p ON p.offering_id=o.id \
              WHERE (o.variant IS NULL OR o.variant='') AND p.micros_per_unit > 0 \
                AND o.status='live' \
              GROUP BY 1",
        )?;
        let rows = q.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<f64>>(2)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (eid, i, o) = row?;
            if let (Some(i), Some(o)) = (i, o) {
                out.insert(eid, (i / 1e6, o / 1e6));
            }
        }
        Ok(out)
    }

    fn seller_counts(&self) -> Result<HashMap<String, i64>> {
        let mut q = self
            .conn
            .prepare("SELECT entity_id, COUNT(DISTINCT provider_id) FROM offerings GROUP BY 1")?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn makers(&self) -> Result<HashMap<String, String>> {
        let mut q = self.conn.prepare(
            "SELECT e.id, COALESCE(p.name, e.maker) FROM entities e \
               LEFT JOIN providers p ON p.id = e.maker WHERE e.maker IS NOT NULL",
        )?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// What each thing is filed under, so a niche can refuse a pick that does
    /// not do the work it is named for.
    fn tasks_of(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut q = self.conn.prepare(
            "SELECT id, json_extract(attrs,'$.tasks') FROM entities \
              WHERE json_extract(attrs,'$.tasks') IS NOT NULL",
        )?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows {
            let (id, json) = row?;
            if let Ok(v) = serde_json::from_str::<Vec<String>>(&json) {
                out.insert(id, v);
            }
        }
        Ok(out)
    }

    fn licences(&self) -> Result<HashMap<String, String>> {
        let mut q = self.conn.prepare(
            "SELECT id, json_extract(attrs,'$.license') FROM entities \
              WHERE json_extract(attrs,'$.license') IS NOT NULL",
        )?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Models whose weights are published without a licence name to go with
    /// it. models.dev states the fact on every model it lists; nobody there
    /// states the terms, so the terms stay unread rather than guessed.
    fn published_weights(&self) -> Result<std::collections::HashSet<String>> {
        let mut q = self.conn.prepare(
            "SELECT id FROM entities WHERE json_extract(attrs,'$.open_weights') = 1",
        )?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_niche_names_boards_and_a_unit() {
        for n in NICHES {
            // A half of the market has no boards of its own on purpose: it
            // borrows every board the token-priced niches use, so that the
            // two lists cannot drift apart.
            assert!(
                !n.boards.is_empty() || n.half.is_some(),
                "{} has no boards and is not a half of the market",
                n.key
            );
            assert!(!unit_words(n.unit).is_empty(), "{} has an unknown unit", n.key);
            assert!(n.question.starts_with("which"), "{} asks no question", n.key);
        }
    }

    #[test]
    fn niche_keys_are_addressable_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for n in NICHES {
            assert_eq!(address_slug(n.key), n.key, "{} is not its own address", n.key);
            assert!(seen.insert(n.key), "{} appears twice", n.key);
        }
    }
}

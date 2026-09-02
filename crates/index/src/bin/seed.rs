//! Seed the catalogue with fixtures so the structure can be played with
//! before the crawler exists. Every figure carries `seed://fixture` as its
//! source — the browser page labels the whole set as fixtures, per the
//! product rule of never presenting invented data as fact.

use index::{Entity, Index, Provider};

const SRC: &str = "seed://fixture";
const DAY: &str = "2026-08-24";

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "index.db".into());
    let ix = Index::open(&path)?;

    for (id, name, kind, url) in [
        ("prov_anthropic", "Anthropic", "vendor", "https://anthropic.com"),
        ("prov_openai", "OpenAI", "vendor", "https://openai.com"),
        ("prov_google", "Google", "vendor", "https://ai.google.dev"),
        ("prov_openrouter", "OpenRouter", "aggregator", "https://openrouter.ai"),
        ("prov_bedrock", "AWS Bedrock", "cloud", "https://aws.amazon.com/bedrock"),
        ("prov_groq", "Groq", "host", "https://groq.com"),
        ("prov_serper", "Serper", "vendor", "https://serper.dev"),
        ("prov_brave", "Brave Search", "vendor", "https://brave.com/search/api"),
        ("prov_deepgram", "Deepgram", "vendor", "https://deepgram.com"),
        ("prov_vapi", "Vapi", "vendor", "https://vapi.ai"),
        ("prov_forge", "Forge", "vendor", "https://example.com/forge"),
        ("prov_firecrawl", "Firecrawl", "vendor", "https://firecrawl.dev"),
    ] {
        ix.upsert_provider(&Provider {
            id: id.into(),
            name: name.into(),
            kind: Some(kind.into()),
            url: Some(url.into()),
            notes: None,
        })?;
    }

    let entity = |id: &str, register: &str, name: &str, maker: &str, family: &str,
                  version: &str, derived: Option<&str>, input: &str, output: &str, attrs: &str|
     -> Entity {
        Entity {
            id: id.into(),
            register: register.into(),
            name: name.into(),
            maker: Some(maker.into()),
            family: Some(family.into()),
            version: Some(version.into()),
            derived_from: derived.map(Into::into),
            input_kind: input.into(),
            output_kind: output.into(),
            attrs: attrs.into(),
        }
    };

    // Models.
    ix.insert_entity(&entity(
        "ent_claude-opus-5", "model", "Claude Opus 5", "prov_anthropic", "claude",
        "opus-5", None, "text", "text", r#"{"context":500000,"modalities":"text","license":"proprietary"}"#,
    ))?;
    ix.insert_entity(&entity(
        "ent_gpt-5.6", "model", "GPT-5.6", "prov_openai", "gpt",
        "5.6", None, "text", "text", r#"{"context":400000,"modalities":"text","license":"proprietary"}"#,
    ))?;
    ix.insert_entity(&entity(
        "ent_gemma-4-e2b", "model", "Gemma 4 e2b", "prov_google", "gemma",
        "4-e2b", None, "text", "text", r#"{"context":128000,"modalities":"text","license":"open","weights_open":true}"#,
    ))?;
    ix.insert_entity(&entity(
        "ent_gemma-4-e2b-med", "model", "Gemma 4 e2b Med", "prov_google", "gemma",
        "4-e2b-med", Some("ent_gemma-4-e2b"), "text", "text", r#"{"context":128000,"license":"open","note":"medical finetune"}"#,
    ))?;

    // Tools.
    ix.insert_entity(&entity(
        "ent_web-search", "tool", "Web search", "prov_serper", "search",
        "v1", None, "query", "result links", "{}",
    ))?;
    ix.insert_entity(&entity(
        "ent_page-render", "tool", "Page render", "prov_firecrawl", "render",
        "v1", None, "url", "markdown", "{}",
    ))?;
    ix.insert_entity(&entity(
        "ent_transcribe", "tool", "Transcription", "prov_deepgram", "speech",
        "nova", None, "audio", "text", "{}",
    ))?;

    // Agents.
    ix.insert_entity(&entity(
        "ent_voice-caller", "agent", "Voice caller", "prov_vapi", "voice",
        "v1", None, "goal + phone number", "call outcome + transcript",
        r#"{"job_kinds":"outbound calls"}"#,
    ))?;
    ix.insert_entity(&entity(
        "ent_code-fixer", "agent", "Code fixer", "prov_forge", "coding",
        "v1", None, "repository + issue", "merged patch",
        r#"{"job_kinds":"bug fixes"}"#,
    ))?;

    // Offerings with price components. A helper keeps each line one sighting.
    let offer = |e: &str, p: &str, way: &str, variant: &str, comps: &[(&str, i64)]| -> anyhow::Result<i64> {
        let off = ix.upsert_offering(e, p, way, variant, DAY)?;
        for (dim, micros) in comps {
            ix.add_price(off, dim, *micros, SRC, DAY)?;
        }
        Ok(off)
    };

    // Claude Opus 5 — maker first, then the resellers; a price history on the maker.
    let claude_api = offer("ent_claude-opus-5", "prov_anthropic", "api", "", &[])?;
    ix.add_price(claude_api, "mtok_in", 5_500_000, SRC, "2026-08-01")?;
    ix.add_price(claude_api, "mtok_in", 5_000_000, SRC, DAY)?;
    ix.add_price(claude_api, "mtok_out", 25_000_000, SRC, DAY)?;
    ix.add_metric(claude_api, "tokens_per_second", 60.0, SRC, DAY)?;
    ix.add_metric(claude_api, "ttft_ms", 800.0, SRC, DAY)?;
    offer("ent_claude-opus-5", "prov_openrouter", "aggregator", "", &[("mtok_in", 5_250_000), ("mtok_out", 26_250_000)])?;
    offer("ent_claude-opus-5", "prov_bedrock", "cloud", "", &[("mtok_in", 5_500_000), ("mtok_out", 27_500_000)])?;
    ix.bind_alias("prov_openrouter", "anthropic/claude-opus-5", "ent_claude-opus-5")?;
    ix.bind_alias("prov_bedrock", "us.anthropic.claude-opus-5", "ent_claude-opus-5")?;

    offer("ent_gpt-5.6", "prov_openai", "api", "", &[("mtok_in", 3_000_000), ("mtok_out", 12_000_000)])?;
    offer("ent_gpt-5.6", "prov_openrouter", "aggregator", "", &[("mtok_in", 3_150_000), ("mtok_out", 12_600_000)])?;
    ix.bind_alias("prov_openrouter", "openai/gpt-5.6", "ent_gpt-5.6")?;

    let gemma_groq = offer("ent_gemma-4-e2b", "prov_groq", "api", "", &[("mtok_in", 150_000), ("mtok_out", 300_000)])?;
    ix.add_metric(gemma_groq, "tokens_per_second", 900.0, SRC, DAY)?;
    offer("ent_gemma-4-e2b", "prov_openrouter", "aggregator", "", &[("mtok_in", 180_000), ("mtok_out", 360_000)])?;
    // Open weights on your own hardware: a real offering with no declared price.
    offer("ent_gemma-4-e2b", "prov_google", "local", "Q4", &[])?;
    offer("ent_gemma-4-e2b-med", "prov_groq", "api", "", &[("mtok_in", 200_000), ("mtok_out", 400_000)])?;

    offer("ent_web-search", "prov_serper", "api", "", &[("call", 1_000)])?;
    offer("ent_web-search", "prov_brave", "api", "", &[("call", 500)])?;
    offer("ent_page-render", "prov_firecrawl", "api", "", &[("call", 2_000), ("second", 500)])?;
    offer("ent_transcribe", "prov_deepgram", "api", "", &[("minute", 4_300)])?;
    ix.bind_alias("prov_firecrawl", "scrape", "ent_page-render")?;

    let voice = offer("ent_voice-caller", "prov_vapi", "api", "", &[("minute", 50_000), ("call", 10_000)])?;
    ix.add_metric(voice, "seconds_per_call_p50", 210.0, SRC, DAY)?;
    let fixer = offer("ent_code-fixer", "prov_forge", "api", "", &[("attempt", 2_000_000), ("result", 8_000_000)])?;
    ix.add_metric(fixer, "acceptance_rate", 0.72, SRC, DAY)?;

    println!(
        "seeded: {} providers, {} entities, {} offerings, {} price rows, {} metric rows, {} aliases",
        ix.count("providers")?, ix.count("entities")?, ix.count("offerings")?,
        ix.count("prices")?, ix.count("metrics")?, ix.count("aliases")?,
    );
    Ok(())
}

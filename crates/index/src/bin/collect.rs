//! One collector run over every machine-readable source: OpenRouter's model
//! list, AWS's Bedrock offer file and Azure's Retail Prices API. Bound
//! aliases land as offerings with declared price components (real source_url
//! + date on every row); unknown aliases go to `unmatched_listings` for
//! `mint` to resolve. A failing source reports and the run moves on — data
//! collection may fail open. Run it again any time — a stable market writes
//! nothing.
//!
//! Usage: collect [db-path]   (default: index.db)

use index::{collector, Index, Provider};

const OPENROUTER_MODELS: &str = "https://openrouter.ai/api/v1/models";
const BEDROCK_OFFER: &str =
    "https://pricing.us-east-1.amazonaws.com/offers/v1.0/aws/AmazonBedrock/current/us-east-1/index.json";
const NOVITA_MODELS: &str = "https://api.novita.ai/v3/openai/models";
const CHUTES_MODELS: &str = "https://llm.chutes.ai/v1/models";
const REQUESTY_MODELS: &str = "https://router.requesty.ai/v1/models";
const DEEPINFRA_MODELS: &str = "https://api.deepinfra.com/models/list";
const NOUS_MODELS: &str = "https://inference-api.nousresearch.com/v1/models";
const SAMBANOVA_MODELS: &str = "https://api.sambanova.ai/v1/models";
const NEBIUS_MODELS: &str = "https://tokenfactory.nebius.com/api/public/models_info";
const HF_ROUTER_MODELS: &str = "https://router.huggingface.co/v1/models";
const VERCEL_MODELS: &str = "https://ai-gateway.vercel.sh/v1/models";
// The gateways `gateways.py` read that this did not. Each publishes the
// OpenAI models list with its own names for the fields, so the difference is
// a table rather than a parser.
const ELECTRONHUB_MODELS: &str = "https://api.electronhub.ai/v1/models";
const INFERENCENET_MODELS: &str = "https://api.inference.net/v1/models";
const AVIAN_MODELS: &str = "https://api.avian.io/v1/models";
const PPINFRA_MODELS: &str = "https://api.ppinfra.com/openai/v1/models";

const AZURE_PRICES: &str = "https://prices.azure.com/api/retail/prices?$filter=serviceName eq 'Foundry Models' and armRegionName eq 'eastus' and priceType eq 'Consumption'";

async fn fetch(client: &reqwest::Client, url: &str) -> anyhow::Result<serde_json::Value> {
    Ok(client
        .get(url)
        .header("User-Agent", "pass-index-collector")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn openrouter(client: &reqwest::Client, ix: &Index, today: &str) -> anyhow::Result<String> {
    let body = fetch(client, OPENROUTER_MODELS).await?;
    let mut listings = collector::parse_openrouter(&body);
    // The model list prices only the route OpenRouter picks by default. Each
    // model's endpoint list prices every upstream that serves it, which is
    // what the market actually offers — walk them, but never let one dead
    // upstream page cost the whole run.
    let ids: Vec<String> = listings.iter().map(|l| l.alias.clone()).collect();
    let (mut walked, mut failed) = (0usize, 0usize);
    for id in &ids {
        let url = format!("https://openrouter.ai/api/v1/models/{id}/endpoints");
        match fetch(client, &url).await {
            Ok(page) => {
                listings.extend(collector::parse_openrouter_endpoints(&page));
                walked += 1;
            }
            Err(_) => failed += 1,
        }
    }
    let s = collector::apply(ix, "prov_openrouter", "aggregator", OPENROUTER_MODELS, today, &listings)?;
    Ok(format!(
        "{} listings from {} models ({walked} endpoint lists read, {failed} unreadable) — {s}",
        listings.len(),
        ids.len()
    ))
}

/// Every source below publishes its own price list as JSON, so one shape of
/// code fits them all: fetch, parse, apply. What the source calls a model
/// stays its own string — binding is the operator's job, through `mint`.
async fn listing_source(
    client: &reqwest::Client,
    ix: &Index,
    today: &str,
    provider: &str,
    way: &str,
    url: &str,
    parse: fn(&serde_json::Value) -> Vec<collector::Listing>,
) -> anyhow::Result<String> {
    let body = fetch(client, url).await?;
    let listings = parse(&body);
    let s = collector::apply(ix, provider, way, url, today, &listings)?;
    Ok(format!("{} listings — {s}", listings.len()))
}

/// A gateway whose rate card differs from the others only in what it calls
/// its fields and which unit it prints them in.
///
/// These bind through the resolver rather than through `collector::apply`.
/// A gateway resells other people's models under its own spelling, so almost
/// nothing it lists has been minted under that exact string; sent down the
/// deliberate path, 546 correctly parsed listings wrote nothing at all and
/// went to the pen instead.
async fn flat_source(
    client: &reqwest::Client,
    ix: &Index,
    today: &str,
    provider: &str,
    url: &str,
    scale: f64,
    keys: &[(&str, &str)],
) -> anyhow::Result<String> {
    let body = fetch(client, url).await?;
    priced(ix, provider, url, today, collector::parse_flat(&body, scale, keys)).await
}

async fn ppinfra_source(
    client: &reqwest::Client,
    ix: &Index,
    today: &str,
) -> anyhow::Result<String> {
    let body = fetch(client, PPINFRA_MODELS).await?;
    priced(ix, "prov_ppinfra", PPINFRA_MODELS, today, collector::parse_ppinfra(&body)).await
}

/// Turn parsed listings into observations and let the one writer persist them.
async fn priced(
    ix: &Index,
    provider: &str,
    url: &str,
    today: &str,
    listings: Vec<collector::Listing>,
) -> anyhow::Result<String> {
    let mut r = index::resolve::Resolver::from_conn(ix.conn())?;
    let obs: Vec<index::feed::Observation> = listings
        .iter()
        .map(|l| index::feed::Observation {
            subject: l.alias.clone(),
            source_url: url.to_string(),
            payload: l.components.clone(),
            seller: provider.to_string(),
        })
        .collect();
    let (bound, wrote) =
        index::feed::write_prices(ix.conn(), &obs, &mut r, today, "aggregator")?;
    Ok(format!("{} listings, {bound} bound, {wrote} figures", listings.len()))
}

async fn bedrock(client: &reqwest::Client, ix: &Index, today: &str) -> anyhow::Result<String> {
    let offer = fetch(client, BEDROCK_OFFER).await?;
    let listings = collector::parse_bedrock(&offer);
    let s = collector::apply(ix, "prov_bedrock", "cloud", BEDROCK_OFFER, today, &listings)?;
    Ok(format!("{} listings — {s}", listings.len()))
}

async fn azure(client: &reqwest::Client, ix: &Index, today: &str) -> anyhow::Result<String> {
    let mut items = Vec::new();
    let mut url = AZURE_PRICES.to_string();
    for _ in 0..100 {
        let mut page = fetch(client, &url).await?;
        if let Some(arr) = page["Items"].as_array_mut() {
            items.append(arr);
        }
        match page["NextPageLink"].as_str() {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
    }
    let listings = collector::parse_azure(&items);
    // The API paginates; the stable citation is the filter URL itself.
    let s = collector::apply(ix, "prov_azure", "cloud", AZURE_PRICES, today, &listings)?;
    Ok(format!("{} meters, {} listings — {s}", items.len(), listings.len()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db = std::env::args().nth(1).unwrap_or_else(|| "index.db".into());
    let ix = Index::open(&db)?;
    let today = collector::today_utc();
    let client = reqwest::Client::new();

    for (id, name, url, kind) in [
        ("prov_openrouter", "OpenRouter", "https://openrouter.ai", "aggregator"),
        ("prov_bedrock", "AWS Bedrock", "https://aws.amazon.com/bedrock", "cloud"),
        ("prov_azure", "Microsoft Azure AI", "https://azure.microsoft.com", "cloud"),
        ("prov_novita", "Novita AI", "https://novita.ai", "aggregator"),
        ("prov_chutes", "Chutes", "https://chutes.ai", "aggregator"),
        ("prov_requesty", "Requesty", "https://requesty.ai", "aggregator"),
        ("prov_deepinfra", "DeepInfra", "https://deepinfra.com", "aggregator"),
        ("prov_nous", "Nous Research", "https://nousresearch.com", "aggregator"),
        ("prov_sambanova", "SambaNova Cloud", "https://cloud.sambanova.ai", "aggregator"),
        ("prov_nebius", "Nebius Token Factory", "https://tokenfactory.nebius.com", "aggregator"),
        ("prov_huggingface", "Hugging Face Inference Providers", "https://huggingface.co", "aggregator"),
        ("prov_vercel", "Vercel AI Gateway", "https://vercel.com/ai-gateway", "aggregator"),
    ] {
        ix.upsert_provider(&Provider {
            id: id.into(),
            name: name.into(),
            url: Some(url.into()),
            kind: Some(kind.into()),
            notes: None,
        })?;
    }

    let mut failures = 0;
    let sources = [
        ("openrouter", openrouter(&client, &ix, &today).await),
        ("bedrock", bedrock(&client, &ix, &today).await),
        ("azure", azure(&client, &ix, &today).await),
        ("novita", listing_source(&client, &ix, &today, "prov_novita", "aggregator",
            NOVITA_MODELS, collector::parse_novita).await),
        ("chutes", listing_source(&client, &ix, &today, "prov_chutes", "aggregator",
            CHUTES_MODELS, collector::parse_chutes).await),
        ("requesty", listing_source(&client, &ix, &today, "prov_requesty", "aggregator",
            REQUESTY_MODELS, collector::parse_requesty).await),
        ("deepinfra", listing_source(&client, &ix, &today, "prov_deepinfra", "aggregator",
            DEEPINFRA_MODELS, collector::parse_deepinfra).await),
        ("nous", listing_source(&client, &ix, &today, "prov_nous", "aggregator",
            NOUS_MODELS, collector::parse_openrouter).await),
        ("sambanova", listing_source(&client, &ix, &today, "prov_sambanova", "aggregator",
            SAMBANOVA_MODELS, collector::parse_sambanova).await),
        ("nebius", listing_source(&client, &ix, &today, "prov_nebius", "aggregator",
            NEBIUS_MODELS, collector::parse_nebius).await),
        ("huggingface", listing_source(&client, &ix, &today, "prov_huggingface", "aggregator",
            HF_ROUTER_MODELS, collector::parse_hf_router).await),
        ("electronhub", flat_source(&client, &ix, &today, "prov_electronhub",
            ELECTRONHUB_MODELS, collector::PER_MILLION,
            &[("input", "mtok_in"), ("output", "mtok_out")]).await),
        ("inference-net", flat_source(&client, &ix, &today, "prov_inferencenet",
            INFERENCENET_MODELS, collector::PER_TOKEN,
            &[("prompt", "mtok_in"), ("completion", "mtok_out"),
              ("input_cache_read", "mtok_cache_read")]).await),
        ("avian", flat_source(&client, &ix, &today, "prov_avian",
            AVIAN_MODELS, collector::PER_MILLION,
            &[("input_per_million", "mtok_in"), ("output_per_million", "mtok_out"),
              ("cache_read_per_million", "mtok_cache_read")]).await),
        ("ppinfra", ppinfra_source(&client, &ix, &today).await),
        ("vercel", listing_source(&client, &ix, &today, "prov_vercel", "aggregator",
            VERCEL_MODELS, collector::parse_vercel).await),
    ];
    let total = sources.len();
    for (label, result) in sources {
        match result {
            Ok(line) => println!("{label}: {line}"),
            Err(e) => {
                failures += 1;
                println!("{label}: FAILED — {e}");
            }
        }
    }
    if failures == total {
        anyhow::bail!("every source failed");
    }
    let swept = ix.drop_empty_offerings()?;
    if swept > 0 {
        println!("swept {swept} offerings left without a price");
    }
    Ok(())
}

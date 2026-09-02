//! One answer to "what does this name refer to", for every collector.
//!
//! Seven copies of this logic grew across four files, each slightly different,
//! and the differences were not decisions — they were whichever rules the
//! author happened to need that afternoon. So a name that bound in one
//! collector missed in the next, and a model already in the catalogue arrived
//! again under a seller's spelling of it.
//!
//! Binding wrong is worse than missing: a price on the wrong model is a lie
//! with a citation. So every rule here removes something a seller added and
//! nothing else, and what remains must equal a name the catalogue already
//! answers to. Nothing is guessed at, nothing is fuzzy-matched, and a miss is
//! reported.
//!
//! This is the Rust port of what was `tools/index/resolve.py`. The rules are
//! carried over unchanged and the tests below assert the shapes the Python
//! produced, because the one thing a rewrite of this file must not do is
//! quietly bind something differently.

use crate::Index;
use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Vendor prefixes a source bolts on, which the catalogue keeps in `maker`.
pub const VENDORS: &[&str] = &[
    "openai", "anthropic", "google", "xai", "meta", "mistral", "deepseek",
    "alibaba", "qwen", "moonshot", "zai", "z-ai", "minimax", "bytedance",
    "amazon", "microsoft", "nvidia", "cohere", "ai21", "stepfun", "baidu",
    "ibm-granite", "wan-ai", "tencent", "01-ai", "upstage", "perplexity",
    "thinky", "thinkingmachines", "liquid", "poolside", "inclusionai",
    "bytedance-seed", "fireworks", "chutes", "together", "groq", "cerebras",
    "novita", "deepinfra", "nousresearch", "writer", "twelvelabs", "reka",
    "allenai", "mistralai", "meta-llama", "google-deepmind", "openrouter",
    // The clouds put their own name in front of somebody else's model.
    "azure", "aws", "bedrock", "vertex", "gcp", "vertex-ai", "azure-openai",
    "azureai", "oci", "watsonx", "sagemaker", "databricks", "snowflake",
    "cloudflare", "workers-ai", "sambanova", "baseten", "replicate",
    "hyperbolic", "lambda", "nebius", "scaleway", "ovh", "vercel",
];

/// Reasoning effort, which is a lane and not a model.
/// The settings a seller appends to a listing. "thinking" and "reasoning"
/// are not here: they name a different product, not a dial on this one —
/// stripping them priced thinking models onto their base cards.
const EFFORTS: &[&str] = &[
    "low", "medium", "high", "xhigh", "max", "minimal", "none",
    "nothinking", "no-thinking",
];

/// Suffixes naming a release channel, a route or a region rather than a thing.
const CHANNELS: &[&str] = &[
    "preview", "exp", "experimental", "beta", "latest", "stable", "openrouter",
    "bedrock", "turbo-preview", "free",
];

const REGIONS: &[&str] = &["us", "eu", "apac", "au", "global", "uk", "jp"];

/// Words too common to be a key on their own. "Large" is what is left of
/// "Mistral Large" once the maker comes off, and it would happily claim any
/// other company's Large.
const TOO_GENERIC: &[&str] = &[
    "large", "medium", "small", "mini", "nano", "flash", "pro", "max", "lite",
    "turbo", "base", "chat", "code", "coder", "instruct", "ocr", "embed",
    "embedding", "rerank", "vision", "audio", "video", "image", "search",
    "r1", "v1", "v2", "v3", "v4", "preview", "latest", "standard",
];

pub fn too_generic(f: &str) -> bool {
    TOO_GENERIC.contains(&f)
}

/// The comparison form: letters and digits, nothing else.
///
/// A plus is spelled out rather than dropped. Command R and Command R+ are
/// different models at different prices, and reducing both to "commandr" put
/// one of them's price on the other.
pub fn norm(s: &str) -> String {
    s.to_lowercase()
        .replace('+', "plus")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

struct Patterns {
    iso_date: Regex,
    compact_date: Regex,
    thinking_budget: Regex,
    context_budget: Regex,
    colon_number: Regex,
    version_suffix: Regex,
    channel_suffix: Regex,
    effort_suffix: Regex,
    vendor_prefix: Regex,
    label_prefix: Regex,
    dotted_prefix: Regex,
    region_prefix: Regex,
    trailing_aside: Regex,
    row_caption: Regex,
    board_tail: Regex,
    lane_word: Regex,
    dated_preview: Regex,
    swap_num_word: Regex,
    swap_word_num: Regex,
    p_version: Regex,
    trailing_nought: Regex,
}

fn pats() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        // A date is a snapshot marker wherever it stands. Anchored to the
        // end, "claude-opus-4-5-20251101-thinking" kept its date once
        // thinking stopped being stripped: the marker sat mid-name and
        // nothing could reach it. The eight-digit form is constrained to
        // this decade so a genuine number in a name cannot be eaten.
        iso_date: Regex::new(r"[-_]\d{4}-\d{2}-\d{2}([-_]|$)").unwrap(),
        compact_date: Regex::new(r"[-_]20[23]\d{5}([-_]|$)").unwrap(),
        thinking_budget: Regex::new(r"(?i)[-_]thinking[-_]\d+k$").unwrap(),
        context_budget: Regex::new(r"(?i)[-_]\d+k$").unwrap(),
        colon_number: Regex::new(r":\d+$").unwrap(),
        version_suffix: Regex::new(r"-v\d+(:\d+)?$").unwrap(),
        channel_suffix: Regex::new(&format!(r"(?i)[-_:]({})$", CHANNELS.join("|"))).unwrap(),
        effort_suffix: Regex::new(&format!(r"(?i)[(_-]({})\)?$", EFFORTS.join("|"))).unwrap(),
        vendor_prefix: Regex::new(&format!(r"(?i)^({})[-_/ ]", VENDORS.join("|"))).unwrap(),
        label_prefix: Regex::new(r"^[A-Za-z0-9 .]{1,24}:\s*").unwrap(),
        dotted_prefix: Regex::new(r"^[a-z0-9_]+\.").unwrap(),
        region_prefix: Regex::new(&format!(r"(?i)^({})[.\-]", REGIONS.join("|"))).unwrap(),
        trailing_aside: Regex::new(r"\s*\([^)]*\)\s*$").unwrap(),
        row_caption: Regex::new(r"\s+(Model|Agent|System)\s*$").unwrap(),
        board_tail: Regex::new(r"\s+[^\s·]+(?:\s+[^\s·]+)?\s+·\s+\S+\s*$").unwrap(),
        lane_word: Regex::new(r"(?i)[-_](unknown|default|standard)$").unwrap(),
        dated_preview: Regex::new(r"(?i)[-_]preview[-_]\d{2}[-_]\d{2}$").unwrap(),
        swap_num_word: Regex::new(r"^(.*?)\s+(\d+(?:\.\d+)*)\s+([A-Za-z][\w.-]*)\s*$").unwrap(),
        swap_word_num: Regex::new(r"^(.*?)\s+([A-Za-z][\w.-]*)\s+(\d+(?:\.\d+)*)\s*$").unwrap(),
        p_version: Regex::new(r"(\d)p(\d)").unwrap(),
        trailing_nought: Regex::new(r"(\d)\.0($|\D)").unwrap(),
    })
}

/// Take the lane suffixes off a name and say which effort was found.
///
/// A source writes them in whatever order it likes —
/// `openai-gpt-5-6-luna-max-2026-07-30` puts the date last,
/// `claude-opus-4-5-20251101-thinking-32k` a thinking budget — so they come
/// off in a loop rather than once.
pub fn strip_lanes(raw: &str) -> (String, Option<String>) {
    let p = pats();
    let mut s = raw.trim().to_string();
    let mut effort: Option<String> = None;
    for _ in 0..8 {
        let before = s.clone();
        s = p.iso_date.replace(&s, "$1").to_string();
        s = p.compact_date.replace(&s, "$1").to_string();
        s = p.thinking_budget.replace(&s, "-thinking").to_string();
        // A context budget a board ran the model at — "claude-opus-4-6_120K",
        // "claude-sonnet-4-5-20250929_32K". It is a setting, not a model, and
        // it accounted for fifty misses on Epoch's boards alone.
        s = p.context_budget.replace(&s, "").to_string();
        s = p.colon_number.replace(&s, "").to_string();
        s = p.version_suffix.replace(&s, "").to_string();
        s = p.channel_suffix.replace(&s, "").to_string();
        // An effort is stripped only where a source appends it as a lane —
        // after a hyphen, an underscore or into a bracket. Never after a
        // space: "Kimi K2.5 Thinking" and "Qwen3 Max" are models, and eating
        // the last word of them merged three pairs of different things.
        if let Some(m) = p.effort_suffix.find(&s) {
            let (head, tail) = s.split_at(m.start());
            if effort.is_none() {
                let word = p
                    .effort_suffix
                    .captures(tail)
                    .and_then(|c| c.get(1).map(|g| g.as_str().to_lowercase()));
                effort = word;
            }
            s = head.trim_matches(|c| c == ' ' || c == '-' || c == '_' || c == '(').to_string();
        }
        if s == before {
            break;
        }
    }
    (s, effort)
}

/// Move a version number to the other side of the word that follows it.
///
/// Written out because it is the one rule here that reorders rather than
/// strips: three words, the middle one a number, and the other two a family
/// and a tier. Whichever order a seller wrote, the other is tried too.
pub fn swap_version(s: &str) -> String {
    let p = pats();
    if let Some(c) = p.swap_num_word.captures(s) {
        return format!("{} {} {}", &c[1], &c[3], &c[2]);
    }
    if let Some(c) = p.swap_word_num.captures(s) {
        return format!("{} {} {}", &c[1], &c[3], &c[2]);
    }
    s.to_string()
}

fn drop_trailing_nought(s: &str) -> String {
    pats().trailing_nought.replace_all(s, "$1$2").to_string()
}

/// Every shape of a name that should mean the same thing, nearest first.
///
/// **The order is part of the answer.** A name often reaches two entities at
/// once — a model and its own thinking lane, a card and a dated snapshot of
/// it. Handing back a set let whichever the hash visited first win, which is
/// not a decision: it differed from the Python this was ported from on 105 of
/// the catalogue's names, and could have differed between two runs of either.
/// Seven of them put Stability's, MiniMax's and OpenChat's prices on GPT-5.
///
/// So the shapes come back ranked by how far they are from what was written.
///
/// The rules compose. Each one strips something a source added, and stripping
/// one often uncovers another: `us.amazon.nova-2-pro-preview-20251202-v1:0`
/// needs a region, a vendor, a channel, a date and a version taken off, in
/// that order, and an earlier version of this applied each rule to the
/// original and never to the result. It produced six near-misses and no hit.
pub fn forms(raw: &str) -> Vec<String> {
    let p = pats();
    let mut out: Vec<String> = Vec::new();
    let mut have: HashSet<String> = HashSet::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Breadth first, and nothing seeded but the name itself, so the queue is
    // the ranking: everything one rule away is tried before anything two
    // rules away. Lane-stripping is one of the rules, so the fully stripped
    // base arrives in its own time rather than jumping ahead of rules that
    // take off a single layer.
    let mut queue: Vec<String> = vec![raw.trim().to_string()];

    let mut head = 0usize;
    while head < queue.len() {
        let s = queue[head].trim().to_string();
        head += 1;
        if s.is_empty() || seen.contains(&s) || seen.len() > 60 {
            continue;
        }
        seen.insert(s.clone());
        // "k2p5" is how a repository writes "k2.5", and the two are one
        // model. A trailing nought on a version is decoration: Imagen 4.0
        // Fast and Imagen 4 Fast are one model. Both are spellings of this
        // shape rather than shapes of their own, so they sit here beside it.
        for wide in [
            s.clone(),
            p.p_version.replace_all(&s, "$1.$2").to_string(),
            drop_trailing_nought(&s),
        ] {
            let f = norm(&wide);
            if !f.is_empty() && have.insert(f.clone()) {
                out.push(f);
            }
        }
        // A seller's path tail, but never as a bare dictionary word: the
        // namespace is most of the name's information, and "acme/extract"
        // stripped to "extract" bound one company's lane onto another
        // company's product. A tail with a digit or more than one word still
        // reads as a model.
        let tail = s.rsplit('/').next().unwrap_or(&s).to_string();
        let tail = if s.contains('/')
            && !tail.chars().any(|c| c.is_ascii_digit())
            && !tail.contains([' ', '-', '_', '.'])
        {
            String::new()
        } else {
            tail
        };
        let candidates = [
            tail, // a seller's path
            p.vendor_prefix.replace(&s, "").to_string(),
            p.label_prefix.replace(&s, "").to_string(), // "Anthropic: Claude"
            p.dotted_prefix.replace(&s, "").to_string(), // "anthropic.claude"
            p.region_prefix.replace(&s, "").to_string(),
            p.trailing_aside.replace(&s, "").to_string(), // a trailing aside
            p.row_caption.replace(&s, "").to_string(),    // a row's own caption
            p.board_tail.replace(&s, "").to_string(),
            p.lane_word.replace(&s, "").to_string(),
            p.dated_preview.replace(&s, "").to_string(),
            strip_lanes(&s).0,
            // A version that changed places. "Claude 4.1 Opus" is "Claude Opus
            // 4.1"; sellers pick a side and the catalogue must not mint both.
            swap_version(&s),
        ];
        for shape in candidates {
            if !shape.is_empty() && shape != s {
                queue.push(shape);
            }
        }
    }

    out
}

/// The shapes the catalogue's own name should answer to.
///
/// Deliberately shy. The aggressive reduction belongs on the incoming side:
/// a source's spelling is worked down towards ours, not ours out towards it.
/// Applied in both directions it made Command R and Command R+ the same
/// model, and Qwen3 Max the same as Qwen3 Max Thinking.
///
/// Not even the trailing aside comes off. "Claude Opus 5" and "Claude Opus 5
/// (Fast)" are two rows at two prices, and reducing the second to the first
/// made the key ambiguous — so it was dropped, and both models lost every
/// binding they had.
pub fn index_forms(name: &str) -> HashSet<String> {
    let p = pats();
    let mut out: HashSet<String> = HashSet::new();
    out.insert(name.to_string());
    out.insert(p.vendor_prefix.replace(name, "").to_string());
    // A trailing nought on a version is decoration, not a distinction. Held
    // apart, "Imagen 4.0 Fast" and "Imagen 4 Fast" were two cards for one
    // model at two prices — which the nightly fold could not see, because it
    // compares these forms and they did not meet.
    for x in out.clone() {
        out.insert(drop_trailing_nought(&x));
    }
    out.iter()
        .map(|x| norm(x))
        .filter(|f| !f.is_empty() && !too_generic(f))
        .collect()
}

/// The catalogue's names, indexed once, asked many times.
pub struct Resolver {
    /// The form a name reduces to, and the one entity that answers to it.
    pub by: HashMap<String, String>,
    /// A form two different entities both answer to is worse than no form at
    /// all: first-come-wins is not a decision, and the loser's price lands on
    /// the winner. Ambiguity is dropped, and counted.
    pub ambiguous: HashSet<String>,
    misses: HashMap<String, usize>,
}

impl Resolver {
    pub fn build(ix: &Index) -> Result<Self> {
        Self::from_conn(ix.conn())
    }

    /// The same, for a caller holding the connection rather than the index.
    pub fn from_conn(conn: &rusqlite::Connection) -> Result<Self> {
        let mut claims: HashMap<String, HashSet<String>> = HashMap::new();
        let mut claim = |f: String, eid: &str, claims: &mut HashMap<String, HashSet<String>>| {
            if f.is_empty() || too_generic(&f) {
                return;
            }
            claims.entry(f).or_default().insert(eid.to_string());
        };

        let mut q = conn.prepare("SELECT id, name FROM entities")?;
        let rows: Vec<(String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (eid, name) in rows {
            for f in index_forms(&name) {
                claim(f, &eid, &mut claims);
            }
        }
        let mut q = conn.prepare("SELECT entity_id, alias FROM aliases")?;
        let rows: Vec<(String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        for (eid, alias) in rows {
            let mut fs = index_forms(&alias);
            fs.insert(norm(&alias));
            for f in fs {
                claim(f, &eid, &mut claims);
            }
        }

        let ambiguous: HashSet<String> = claims
            .iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(f, _)| f.clone())
            .collect();
        let by: HashMap<String, String> = claims
            .into_iter()
            .filter(|(_, ids)| ids.len() == 1)
            .map(|(f, ids)| (f, ids.into_iter().next().unwrap()))
            .collect();
        Ok(Resolver { by, ambiguous, misses: HashMap::new() })
    }

    /// The entity this name refers to, or nothing. A miss is remembered.
    pub fn bind(&mut self, name: &str) -> Option<String> {
        for f in forms(name) {
            if too_generic(&f) {
                continue;
            }
            // Ambiguity fails closed. The queue runs near to far, so every
            // later form is a further-stripped reading of the same name;
            // when the nearest reading is claimed by two rows, letting a
            // farther one decide is guessing with less information, and it
            // guessed another maker's card.
            if self.ambiguous.contains(&f) {
                break;
            }
            if let Some(eid) = self.by.get(&f) {
                return Some(eid.clone());
            }
        }
        *self.misses.entry(name.to_string()).or_insert(0) += 1;
        None
    }

    /// Binding without recording a miss, for callers that are only asking.
    pub fn look(&self, name: &str) -> Option<String> {
        for f in forms(name) {
            if too_generic(&f) {
                continue;
            }
            // Ambiguity fails closed. The queue runs near to far, so every
            // later form is a further-stripped reading of the same name;
            // when the nearest reading is claimed by two rows, letting a
            // farther one decide is guessing with less information, and it
            // guessed another maker's card.
            if self.ambiguous.contains(&f) {
                break;
            }
            if let Some(eid) = self.by.get(&f) {
                return Some(eid.clone());
            }
        }
        None
    }

    /// What could not be placed, so it is counted rather than guessed.
    pub fn report(&self, label: &str, limit: usize) -> String {
        if self.misses.is_empty() {
            return format!("{}everything bound", if label.is_empty() {
                String::new()
            } else {
                format!("{label}: ")
            });
        }
        let total: usize = self.misses.values().sum();
        let mut top: Vec<(&String, &usize)> = self.misses.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        let mut lines = vec![format!(
            "{}{} names did not bind, {} occurrences",
            if label.is_empty() { String::new() } else { format!("{label}: ") },
            self.misses.len(),
            total
        )];
        for (n, c) in top.into_iter().take(limit) {
            lines.push(format!("     {:<52} x{}", &n[..n.len().min(52)], c));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes the Python produced, asserted here so the port cannot
    /// quietly bind something differently.
    #[test]
    fn a_plus_is_spelled_out() {
        assert_eq!(norm("Command R+"), "commandrplus");
        assert_ne!(norm("Command R+"), norm("Command R"));
    }

    #[test]
    fn lanes_come_off_in_any_order() {
        assert_eq!(strip_lanes("openai-gpt-5-6-luna-max-2026-07-30").0, "openai-gpt-5-6-luna");
        assert_eq!(strip_lanes("claude-opus-4-5-20251101-thinking-32k").0,
                   "claude-opus-4-5-thinking");
        assert_eq!(strip_lanes("claude-opus-4-6_120K").0, "claude-opus-4-6");
    }

    #[test]
    fn an_effort_after_a_space_is_part_of_the_name() {
        // "Kimi K2.5 Thinking" is a model, not a lane of Kimi K2.5.
        assert_eq!(strip_lanes("Kimi K2.5 Thinking").0, "Kimi K2.5 Thinking");
        assert_eq!(strip_lanes("gpt-5-high").0, "gpt-5");
    }

    /// The rule the 105 disagreements came down to: what a seller wrote is
    /// tried before anything stripped out of it, so a model and its own
    /// thinking lane stop depending on which the hash reached first.
    #[test]
    fn the_name_as_written_is_tried_before_anything_stripped_from_it() {
        // A true lane comes off, and the whole spelling is still tried
        // first. Thinking is no longer a lane — it names a product — so the
        // specimen uses an effort setting that is one.
        let f = forms("ERNIE-5.0-high");
        let whole = f.iter().position(|x| x == "ernie50high");
        let stripped = f.iter().position(|x| x == "ernie50");
        assert!(whole.is_some() && stripped.is_some(), "{f:?}");
        assert!(whole < stripped, "{f:?}");
    }

    #[test]
    fn a_thinking_name_never_collapses_onto_its_base() {
        let f = forms("ERNIE-5.0-Thinking");
        assert!(f.iter().any(|x| x == "ernie50thinking"), "{f:?}");
        assert!(!f.iter().any(|x| x == "ernie50"), "{f:?}");
    }

    #[test]
    fn a_sellers_path_reduces_to_our_name() {
        let f = forms("us.amazon.nova-2-pro-preview-20251202-v1:0");
        assert!(f.iter().any(|x| x == "nova2pro"), "{f:?}");
    }

    #[test]
    fn a_version_that_changed_places_is_tried_both_ways() {
        let f = forms("Claude 4.1 Opus");
        assert!(f.iter().any(|x| x == "claudeopus41"), "{f:?}");
    }

    #[test]
    fn a_trailing_nought_is_decoration() {
        assert!(forms("Imagen 4.0 Ultra")
            .iter()
            .any(|x| x == "imagen4ultra"));
        assert!(index_forms("Imagen 4.0 Ultra").contains("imagen4ultra"));
    }

    #[test]
    fn our_own_names_are_not_reduced_aggressively() {
        // Applied in both directions this merged Command R with Command R+,
        // and Qwen3 Max with Qwen3 Max Thinking.
        let f = index_forms("Qwen3 Max Thinking");
        assert!(!f.contains("qwen3max"), "{f:?}");
        assert!(f.contains("qwen3maxthinking"), "{f:?}");
    }

    #[test]
    fn a_word_too_common_is_never_a_key() {
        assert!(index_forms("Mistral Large").iter().all(|f| f != "large"));
    }
}

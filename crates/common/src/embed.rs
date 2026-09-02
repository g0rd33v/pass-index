//! Embedding for the semantic cache and retrieval. The default is a fast
//! local feature-hashing embedder — no network, microsecond latency. Real
//! embedding models (self-hosted preferred, provider fallback — spec §7.4)
//! implement the same trait and swap in via configuration later.

use std::collections::BTreeSet;

/// v2: 512 dims (v1 was 256). Old vectors have a different length, and
/// `cosine` scores mismatched lengths as 0.0 — stale embeddings fail safe
/// as misses. Re-index knowledge bases with `gate kb reindex`.
pub const DIMS: usize = 512;

/// Word features carry more weight than subword features so that sharing
/// whole words dominates, while character n-grams bridge morphology.
const WORD_WEIGHT: f32 = 2.0;
const CHAR_WEIGHT: f32 = 1.0;

pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// Feature-hashing embedder: word unigrams + bigrams, plus character 3- and
/// 4-grams within each word. The character n-grams make it language-neutral
/// in a way word tokens are not: German compounds (*Betriebstemperatur* /
/// *Temperatur*) and Slavic inflection (*удалённая* / *удалённо*) share
/// most of their n-grams even when the surface tokens differ. No language
/// packs, no model downloads.
pub struct HashEmbedder;

impl Embedder for HashEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let tokens: Vec<String> = text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        let mut v = vec![0f32; DIMS];
        for token in &tokens {
            v[slot(token)] += WORD_WEIGHT;
            // Character n-grams (3 and 4) inside the word, char-boundary safe.
            let chars: Vec<char> = token.chars().collect();
            for n in [3usize, 4] {
                if chars.len() > n {
                    for window in chars.windows(n) {
                        let gram: String = window.iter().collect();
                        v[slot(&gram)] += CHAR_WEIGHT;
                    }
                }
            }
        }
        for pair in tokens.windows(2) {
            v[slot(&format!("{} {}", pair[0], pair[1]))] += WORD_WEIGHT;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

fn slot(token: &str) -> usize {
    // FNV-1a; cheap and stable across runs.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in token.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash % DIMS as u64) as usize
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    // Vectors are L2-normalized at embed time; dot product is cosine.
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Entities that must match for a semantic hit (spec §4.3.6): numbers and
/// email-like tokens carry identity/quantity meaning that similarity alone
/// can gloss over.
pub fn extract_entities(text: &str) -> Vec<String> {
    let mut entities: BTreeSet<String> = BTreeSet::new();
    for token in text.split_whitespace() {
        // Trim edge punctuation only — interior '.'/'@' (emails, versions)
        // survive because tokens end alphanumeric.
        let trimmed = token.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.contains('@') || trimmed.chars().any(|c| c.is_ascii_digit()) {
            entities.insert(trimmed.to_lowercase());
        }
    }
    entities.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_scores_one() {
        let a = HashEmbedder.embed("what is the refund policy");
        let b = HashEmbedder.embed("what is the refund policy");
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn punctuation_and_case_are_invisible() {
        let a = HashEmbedder.embed("Explain the refund policy please");
        let b = HashEmbedder.embed("explain the refund policy, please!");
        assert!(cosine(&a, &b) > 0.99);
    }

    #[test]
    fn unrelated_text_scores_low() {
        let a = HashEmbedder.embed("what is the refund policy for enterprise plans");
        let b = HashEmbedder.embed("write a haiku about mountain sunrises");
        assert!(cosine(&a, &b) < 0.3, "{}", cosine(&a, &b));
    }

    // -- Regressions for the two live-benchmark retrieval misses --

    #[test]
    fn german_compounds_bridge_via_char_ngrams() {
        let query = HashEmbedder.embed(
            "Laut dem Dokument: bei welcher Temperatur darf das Gerät maximal betrieben werden?",
        );
        let chunk = HashEmbedder.embed(
            "Die maximale Betriebstemperatur beträgt 45 Grad Celsius, die minimale minus 10 Grad. \
             Das Gerät wiegt 14,5 Kilogramm und trägt bis zu 120 Kilogramm.",
        );
        let score = cosine(&query, &chunk);
        assert!(
            score >= 0.25,
            "German compound query must clear the retrieval threshold, got {score}"
        );
    }

    #[test]
    fn russian_inflection_bridges_via_char_ngrams() {
        let query = HashEmbedder.embed("Согласно документу, в какие дни можно работать удалённо?");
        let chunk = HashEmbedder.embed(
            "Удалённая работа разрешена по средам и пятницам при согласовании с руководителем. \
             Рабочий день начинается в 9:30 и заканчивается в 18:00.",
        );
        let score = cosine(&query, &chunk);
        assert!(
            score >= 0.25,
            "Russian inflected query must clear the retrieval threshold, got {score}"
        );
    }

    #[test]
    fn ngrams_do_not_create_false_relatedness() {
        // Same language, different topic: must stay clearly below the
        // retrieval threshold despite shared function-word n-grams.
        let a = HashEmbedder.embed("Согласно документу, в какие дни можно работать удалённо?");
        let b = HashEmbedder.embed(
            "Рецепт борща: свёкла, капуста, морковь и говядина варятся два часа на медленном огне.",
        );
        assert!(cosine(&a, &b) < 0.25, "{}", cosine(&a, &b));
        let c = HashEmbedder.embed("Based on the context, explain quantum tunneling barriers");
        let d = HashEmbedder.embed("Refund requests take 30 days to process.");
        assert!(cosine(&c, &d) < 0.25, "{}", cosine(&c, &d));
    }

    #[test]
    fn semantic_cache_bar_still_requires_near_identity() {
        // The 0.95 cache threshold must not become reachable by merely
        // related sentences under the n-gram scheme.
        let a = HashEmbedder.embed("Explain the enterprise refund policy please");
        let b = HashEmbedder.embed("Explain the enterprise refund process please");
        assert!(
            cosine(&a, &b) < 0.95,
            "related-but-different must not cache-hit: {}",
            cosine(&a, &b)
        );
    }

    #[test]
    fn entities_capture_numbers_and_emails() {
        let e = extract_entities("Reset the password for bob@example.com, account 4471.");
        assert!(e.contains(&"bob@example.com".to_string()));
        assert!(e.contains(&"4471".to_string()));
        assert_eq!(
            extract_entities("no identifiers here at all"),
            Vec::<String>::new()
        );
    }
}

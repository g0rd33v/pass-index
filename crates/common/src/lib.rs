//! Shared types and utilities for all Pass services.

pub mod embed;
pub mod names;

use tracing_subscriber::EnvFilter;

/// Initialize tracing for a service. `RUST_LOG` overrides the default level.
pub mod db;

pub fn init_tracing(service: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    tracing::info!(service, "starting");
}

/// Task types the Planner classifies requests into (spec §4.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Generation,
    Summarization,
    Rewriting,
    Extraction,
    Classification,
    Translation,
    DocQa,
    Chat,
}

/// Account quality tiers — the floor the Planner may route down to (spec §4.2.5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityTier {
    Economy,
    Balanced,
    Premium,
}

/// Cache outcome reported in `pass` response metadata (spec §4.1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheStatus {
    Miss,
    Exact,
    Semantic,
}

/// Estimated task difficulty (spec §4.2.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Simple,
    Standard,
    Hard,
}

/// Cheap language tag for per-language routing profiles (M37/M49). A
/// classification primitive alongside `TaskType`/`Difficulty`; detection lives
/// in `planner`, but the type is shared so the ports contract can carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ru,
    En,
    Es,
    De,
    Fr,
    Ja,
    Other,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::Ru => "ru",
            Lang::En => "en",
            Lang::Es => "es",
            Lang::De => "de",
            Lang::Fr => "fr",
            Lang::Ja => "ja",
            Lang::Other => "other",
        }
    }
}

/// Model class used for routing floors (spec rule 4.2.5.1). Ordering matters:
/// `Economy < Mid < Frontier`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Economy,
    Mid,
    Frontier,
}

/// Per-key model handling (spec §4.1.3.3): hint mode may substitute an
/// equivalent-or-better model; strict mode uses exactly the requested one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyMode {
    Hint,
    Strict,
}

/// Model pricing in integer micro-USD per million tokens. All economics
/// math stays in integers so analytics, log, and billing reconcile exactly
/// (spec §6.1); floats appear only at display time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pricing {
    pub input_micros_per_mtok: u64,
    pub output_micros_per_mtok: u64,
}

impl Pricing {
    pub fn from_usd(input_usd_per_mtok: f64, output_usd_per_mtok: f64) -> Self {
        Self {
            input_micros_per_mtok: (input_usd_per_mtok.max(0.0) * 1e6).round() as u64,
            output_micros_per_mtok: (output_usd_per_mtok.max(0.0) * 1e6).round() as u64,
        }
    }

    /// Cost of a request in micro-USD, rounded half-up per component.
    pub fn cost_micros(&self, tokens_in: u64, tokens_out: u64) -> u64 {
        let part = |tokens: u64, price: u64| -> u64 {
            ((tokens as u128 * price as u128 + 500_000) / 1_000_000) as u64
        };
        part(tokens_in, self.input_micros_per_mtok) + part(tokens_out, self.output_micros_per_mtok)
    }
}

/// Micro-USD to display dollars.
pub fn micros_to_usd(micros: u64) -> f64 {
    micros as f64 / 1e6
}

/// The current billing period as `YYYY-MM`, UTC.
///
/// One implementation on purpose: this used to exist three times — here, in
/// the CLI (which opened an in-memory SQLite connection just to call
/// `strftime`, falling back to `"1970-01"`), and in the billing tests — with
/// three different failure modes for the same question.
pub fn current_period() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    // Civil-from-days (Hinnant): exact month arithmetic, no leap drift.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}")
}

#[cfg(test)]
mod period_tests {
    #[test]
    fn period_is_a_well_formed_month() {
        let p = super::current_period();
        assert_eq!(p.len(), 7, "{p}");
        let (y, m) = p.split_once('-').unwrap();
        assert!(y.parse::<u32>().unwrap() >= 2024, "{p}");
        let m: u32 = m.parse().unwrap();
        assert!((1..=12).contains(&m), "{p}");
    }
}

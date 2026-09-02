//! Model-name presentation, shared across the seam. Every user-facing surface
//! (console, Telegram, the public roster, receipts) renders a model id into a
//! human name through the same scheme, so the string never drifts between them.

/// Human name from a routing id tail: strips provider path noise, restores
/// dotted versions (k2p6 -> K2.6), and title-cases with vendor-correct acronyms.
pub fn display_name(model: &str) -> String {
    let tail = model.rsplit('/').next().unwrap_or(model);
    // fireworks encodes dots as 'p' between digits
    let mut fixed = String::new();
    let chars: Vec<char> = tail.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == 'p'
            && i > 0
            && i + 1 < chars.len()
            && chars[i - 1].is_ascii_digit()
            && chars[i + 1].is_ascii_digit()
        {
            fixed.push('.');
        } else {
            fixed.push(*c);
        }
    }
    fixed
        .split('-')
        .map(|tok| match tok.to_ascii_lowercase().as_str() {
            "gpt" => "GPT".into(),
            "oss" => "OSS".into(),
            "glm" => "GLM".into(),
            "ai" => "AI".into(),
            "nvfp4" => "NVFP4".into(),
            "deepseek" => "DeepSeek".into(),
            "minimax" => "MiniMax".into(),
            t if t.starts_with("gpt") => tok.replacen("gpt", "GPT", 1),
            // parameter-count suffixes read better upper-cased:
            // 120b -> 120B, a3b -> A3B (MoE active-parameter counts), a22b -> A22B.
            t if t.len() > 1
                && t.ends_with('b')
                && t[..t.len() - 1]
                    .trim_start_matches('a')
                    .chars()
                    .all(|c| c.is_ascii_digit())
                && t[..t.len() - 1].chars().any(|c| c.is_ascii_digit()) =>
            {
                t.to_ascii_uppercase()
            }
            t if t.chars().next().is_some_and(|c| c.is_ascii_digit()) => tok.to_string(),
            _ => {
                let mut cs = tok.chars();
                match cs.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The provider that actually served a request, from its ledger `route`. The
/// **last** segment answered; this is the provider, not the model's vendor.
pub fn provider_label(route: &str) -> Option<String> {
    let served = route.rsplit(", ").next()?.trim();
    // Streamed routes carry a " (stream)" suffix.
    let served = served.split(" (").next().unwrap_or(served);
    let provider = served.split('/').next()?.trim();
    if provider.is_empty() {
        return None;
    }
    Some(provider_name(provider))
}

/// Display label for a provider name as the router knows it.
pub fn provider_name(provider: &str) -> String {
    match provider.to_ascii_lowercase().as_str() {
        "openrouter" => "OpenRouter".to_string(),
        // fireworks model ids start `accounts/fireworks/models/...`
        "fireworks" | "accounts" => "Fireworks".to_string(),
        "openai" => "OpenAI".to_string(),
        "anthropic" => "Anthropic".to_string(),
        other => {
            let mut cs = other.chars();
            match cs.next() {
                Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_labels_name_the_server() {
        assert_eq!(provider_label("openrouter/deepseek/deepseek-v4").as_deref(), Some("OpenRouter"));
        assert_eq!(provider_label("accounts/fireworks/models/x (stream)").as_deref(), Some("Fireworks"));
    }

    #[test]
    fn display_names_read_right() {
        assert_eq!(display_name("accounts/fireworks/models/gpt-oss-120b"), "GPT OSS 120B");
        assert_eq!(display_name("accounts/fireworks/models/kimi-k2p6"), "Kimi K2.6");
        assert_eq!(display_name("deepseek/deepseek-v4-flash"), "DeepSeek V4 Flash");
        assert_eq!(display_name("nvidia/nemotron-nvfp4"), "Nemotron NVFP4");
        assert_eq!(display_name("anthropic/claude-opus-5"), "Claude Opus 5");
    }
}

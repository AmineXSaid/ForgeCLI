//! Known models: aliases, limits, pricing and how each takes thinking.

/// How a model accepts the `thinking` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingStyle {
    /// Thinking is always on; omit the parameter (or send adaptive). Depth via effort.
    AlwaysOn,
    /// Adaptive thinking; may be switched off (`disabled`, or `between_tools`).
    Adaptive { off: OffMode },
    /// Legacy `{type: "enabled", budget_tokens}`.
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffMode {
    Disabled,
    BetweenTools,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub context_window: u64,
    pub max_output: u32,
    /// USD per million tokens.
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub thinking: ThinkingStyle,
    /// Effort levels accepted in `output_config.effort`; empty = unsupported.
    pub effort_levels: &'static [&'static str],
    pub default_effort: Option<&'static str>,
    pub supports_fast_mode: bool,
}

impl ModelInfo {
    /// Prompt-cache write price (5-minute TTL): 1.25x input.
    pub fn cache_write_price(&self) -> f64 {
        self.input_price * 1.25
    }

    pub fn supports_effort(&self) -> bool {
        !self.effort_levels.is_empty()
    }

    /// Cost in USD of a request's usage.
    pub fn cost(&self, u: &forge_types::Usage) -> f64 {
        (u.input_tokens as f64 * self.input_price
            + u.output_tokens as f64 * self.output_price
            + u.cache_read_input_tokens as f64 * self.cache_read_price
            + u.cache_creation_input_tokens as f64 * self.cache_write_price())
            / 1_000_000.0
    }
}

const ALL_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const NO_XHIGH: &[&str] = &["low", "medium", "high", "max"];

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-fable-5-1",
        display_name: "Fable 5.1",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 10.0,
        output_price: 50.0,
        cache_read_price: 0.25,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-fable-5",
        display_name: "Fable 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 10.0,
        output_price: 50.0,
        cache_read_price: 1.0,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-opus-5-5",
        display_name: "Opus 5.5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 4.0,
        output_price: 20.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("medium"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-5",
        display_name: "Opus 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-4-8",
        display_name: "Opus 4.8",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-4-7",
        display_name: "Opus 4.7",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-opus-4-6",
        display_name: "Opus 4.6",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: NO_XHIGH,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-5-5",
        display_name: "Sonnet 5.5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 2.0,
        output_price: 10.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::Adaptive { off: OffMode::BetweenTools },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-5",
        display_name: "Sonnet 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 2.0,
        output_price: 10.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-4-6",
        display_name: "Sonnet 4.6",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 3.0,
        output_price: 15.0,
        cache_read_price: 0.30,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: NO_XHIGH,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-haiku-4-5",
        display_name: "Haiku 4.5",
        context_window: 200_000,
        max_output: 64_000,
        input_price: 1.0,
        output_price: 5.0,
        cache_read_price: 0.10,
        thinking: ThinkingStyle::Budget,
        effort_levels: &[],
        default_effort: None,
        supports_fast_mode: false,
    },
];

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Model used for cheap background work (titles, summaries of web pages).
pub const SMALL_FAST_MODEL: &str = "claude-haiku-4-5";

/// Map an alias (`opus`, `sonnet`, `haiku`, `fable`, `default`) to a model id.
/// Unknown names pass through unchanged so new ids work without a release.
pub fn resolve_model(name: &str) -> String {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    let (base, suffix) = match lower.strip_suffix("[1m]") {
        Some(b) => (b.to_string(), ""),
        None => (lower.clone(), ""),
    };
    let id = match base.as_str() {
        "" | "default" | "opus" | "best" => DEFAULT_MODEL,
        "sonnet" => "claude-sonnet-5-5",
        "haiku" => "claude-haiku-4-5",
        "fable" => "claude-fable-5-1",
        "opusplan" => DEFAULT_MODEL,
        _ => return n.to_string(),
    };
    format!("{id}{suffix}")
}

/// Look up a model by id (or a dated / prefixed variant of one).
pub fn model_info(id: &str) -> Option<&'static ModelInfo> {
    MODELS
        .iter()
        .find(|m| m.id == id)
        .or_else(|| MODELS.iter().filter(|m| id.starts_with(m.id)).max_by_key(|m| m.id.len()))
}

/// Info for a model, falling back to conservative defaults for unknown ids.
pub fn model_info_or_default(id: &str) -> ModelInfo {
    model_info(id).cloned().unwrap_or(ModelInfo {
        id: "unknown",
        display_name: "Custom model",
        context_window: 200_000,
        max_output: 32_000,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: &[],
        default_effort: None,
        supports_fast_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_types::Usage;

    #[test]
    fn aliases_resolve() {
        assert_eq!(resolve_model("opus"), "claude-opus-5-5");
        assert_eq!(resolve_model("Sonnet"), "claude-sonnet-5-5");
        assert_eq!(resolve_model("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(resolve_model("my-local-model"), "my-local-model");
    }

    #[test]
    fn dated_ids_match_their_family() {
        assert_eq!(model_info("claude-haiku-4-5-20251001").unwrap().id, "claude-haiku-4-5");
        assert_eq!(model_info("claude-opus-5").unwrap().id, "claude-opus-5");
        assert!(model_info("gpt-x").is_none());
    }

    #[test]
    fn cost_is_per_million() {
        let m = model_info("claude-opus-5-5").unwrap();
        let u = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, ..Default::default() };
        assert!((m.cost(&u) - 24.0).abs() < 1e-9);
    }
}

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_types::ToolSpec;

use crate::Tool;

/// Tools available to a session, in a stable order (stable order keeps the
/// prompt-cache prefix stable).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    /// Description overrides (from `FORGE_PROMPTS_DIR`).
    descriptions: BTreeMap<String, String>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.retain(|t| t.name() != tool.name());
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().filter(|t| t.is_enabled()).map(|t| t.name().to_string()).collect()
    }

    pub fn set_description(&mut self, tool: &str, text: String) {
        self.descriptions.insert(tool.to_string(), text);
    }

    /// Keep only tools `keep` accepts.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.tools.retain(|t| keep(t.name()));
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .filter(|t| t.is_enabled())
            .map(|t| ToolSpec {
                name: t.name().to_string(),
                description: self.descriptions.get(t.name()).cloned().unwrap_or_else(|| t.description()),
                input_schema: t.input_schema(),
                kind: None,
                extra: Default::default(),
                cache_control: None,
            })
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.iter()
    }
}

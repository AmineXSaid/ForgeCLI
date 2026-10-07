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
        self.tools.iter().find(|t| t.name() == name && t.is_enabled()).cloned()
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use serde_json::{json, Value};

    use super::*;
    use crate::{Tool, ToolContext, ToolOutput};

    struct Switchable(Arc<AtomicBool>);

    #[async_trait::async_trait]
    impl Tool for Switchable {
        fn name(&self) -> &str {
            "Switchable"
        }

        fn description(&self) -> String {
            "on or off".into()
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn is_enabled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
            ToolOutput::text("ran")
        }
    }

    #[test]
    fn disabled_tools_are_hidden_and_cannot_be_called() {
        let on = Arc::new(AtomicBool::new(true));
        let mut r = ToolRegistry::new();
        r.register(Arc::new(Switchable(on.clone())));
        assert!(r.get("Switchable").is_some());
        assert_eq!(r.specs().len(), 1);
        on.store(false, Ordering::SeqCst);
        assert!(r.get("Switchable").is_none(), "a hidden tool can't be called by name");
        assert!(r.specs().is_empty() && r.names().is_empty());
    }
}

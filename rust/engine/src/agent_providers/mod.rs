pub mod claude_code;
pub mod codex;
mod common;
pub mod debug;
pub mod opencode;
pub mod pi;
pub mod types;

pub use claude_code::{ClaudeCodeAgentProvider, ClaudeCodeAgentProviderOptions};
pub use codex::{CodexAgentProvider, CodexAgentProviderOptions};
pub use debug::{generate_debug_value_from_schema, DebugAgentProvider};
pub use opencode::{OpenCodeAgentProvider, OpenCodeAgentProviderOptions};
pub use pi::{PiAgentProvider, PiAgentProviderOptions};
pub use types::*;

pub fn create_agent_provider(name: &str) -> anyhow::Result<Box<dyn AgentProvider>> {
    match name {
        "debug" => Ok(Box::new(DebugAgentProvider::new())),
        "claude-code" => Ok(Box::new(ClaudeCodeAgentProvider::new(Default::default()))),
        "codex" => Ok(Box::new(CodexAgentProvider::new(Default::default()))),
        "opencode" => Ok(Box::new(OpenCodeAgentProvider::new(Default::default()))),
        "pi" => Ok(Box::new(PiAgentProvider::new(Default::default()))),
        other => anyhow::bail!("Unknown agent provider: {other}"),
    }
}

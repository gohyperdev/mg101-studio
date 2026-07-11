//! Rdzeń agentowy — pętla tool-calling, dostawcy Anthropic/OpenAI, autoryzacja,
//! rozliczanie kosztu (ADR-0001). Port `AgentLoop` v1.
//!
//! Podział (jak w całym rdzeniu): **czysta logika** (typy, normalizacja
//! endpointu, budowa/parsowanie żądań, pętla za traitami, cennik) — testowalna
//! bez sieci; **transport HTTP** (reqwest) wchodzi w części 2 jako implementacja
//! [`LlmClient`]. Narzędzia generowane z rejestru komend (E3), spójne dla UI,
//! agenta i MCP.

mod config;
mod cost;
mod run;
mod types;
mod wire;

pub use config::{is_path_approved, normalize_endpoint, AgentConfig, Provider};
pub use cost::{default_pricing, pricing_for, ModelPricing};
pub use run::{run, AgentError, Authorizer, LlmClient, RunOutcome, ToolExecutor, MAX_ITERATIONS};
pub use types::{ChatMessage, LlmResponse, Role, ToolCall, ToolResult, Usage};
pub use wire::{
    build_anthropic_body, build_body, build_openai_body, parse_anthropic_response,
    parse_openai_response, parse_response, WireError,
};

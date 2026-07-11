//! Wspólne typy konwersacji agenta — port `ChatMessage`/`ToolCall`/`ToolResult`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Rola wiadomości.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Wywołanie narzędzia zaproponowane przez model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Map<String, Value>,
}

/// Wynik wykonania narzędzia (zwracany do modelu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

/// Wiadomość w historii konwersacji.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub tool_results: Vec<ToolResult>,
}

impl ChatMessage {
    /// Wiadomość użytkownika (zwykły tekst).
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            tool_results: vec![],
        }
    }

    /// Wiadomość asystenta (tekst).
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: vec![],
            tool_results: vec![],
        }
    }
}

/// Zużycie tokenów (z odpowiedzi modelu).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    /// Sumuje zużycie (akumulacja przez turę/sesję).
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// Znormalizowana odpowiedź modelu (niezależna od dostawcy).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
}

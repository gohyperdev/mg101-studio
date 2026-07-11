//! Serializacja/deserializacja protokołów LLM — port budowy żądań i parsowania
//! odpowiedzi Anthropic i OpenAI z v1. **Czyste** (JSON in/out), bez HTTP —
//! transport wstrzykuje się osobno (część 2), więc format jest testowalny.

use crate::types::{ChatMessage, LlmResponse, Role, ToolCall, Usage};
use mg101_commands::{ToolDefinition, Variant};
use serde_json::{json, Map, Value};

/// Błąd parsowania odpowiedzi modelu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Odpowiedź nie miała oczekiwanego kształtu.
    Malformed(String),
    /// Brak treści (pusta odpowiedź).
    Empty,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Malformed(m) => write!(f, "zniekształcona odpowiedź: {m}"),
            WireError::Empty => write!(f, "pusta odpowiedź modelu"),
        }
    }
}

fn tools_json(variant: Variant, anthropic: bool) -> Vec<Value> {
    ToolDefinition::all(variant)
        .iter()
        .map(|t| {
            if anthropic {
                t.anthropic_format()
            } else {
                t.openai_format()
            }
        })
        .collect()
}

// --- Anthropic ---

/// Buduje ciało żądania Anthropic Messages (port `buildAnthropicRequest`).
pub fn build_anthropic_body(
    model: &str,
    system: &str,
    history: &[ChatMessage],
    variant: Variant,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    for msg in history {
        if !msg.tool_results.is_empty() {
            let blocks: Vec<Value> = msg
                .tool_results
                .iter()
                .map(|r| {
                    let mut b = json!({
                        "type": "tool_result",
                        "tool_use_id": r.tool_use_id,
                        "content": r.content,
                    });
                    if r.is_error {
                        b["is_error"] = json!(true);
                    }
                    b
                })
                .collect();
            messages.push(json!({"role": "user", "content": blocks}));
        } else if !msg.tool_calls.is_empty() {
            let mut blocks: Vec<Value> = Vec::new();
            if !msg.content.is_empty() {
                blocks.push(json!({"type": "text", "text": msg.content}));
            }
            for c in &msg.tool_calls {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": c.id,
                    "name": c.name,
                    "input": Value::Object(c.arguments.clone()),
                }));
            }
            messages.push(json!({"role": "assistant", "content": blocks}));
        } else {
            messages.push(json!({"role": msg.role.as_str(), "content": msg.content}));
        }
    }

    json!({
        "model": model,
        "max_tokens": 4096,
        "system": system,
        "messages": messages,
        "tools": tools_json(variant, true),
    })
}

/// Parsuje odpowiedź Anthropic Messages (port `requestAnthropic` dekodowania).
pub fn parse_anthropic_response(v: &Value) -> Result<LlmResponse, WireError> {
    let content = v
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| WireError::Malformed("brak pola content".into()))?;

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in content {
        match block.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                let name = block
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default();
                let arguments = block
                    .get("input")
                    .and_then(|i| i.as_object())
                    .cloned()
                    .unwrap_or_default();
                tool_calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments,
                });
            }
            _ => {}
        }
    }

    Ok(LlmResponse {
        text: (!text.is_empty()).then_some(text),
        tool_calls,
        usage: parse_usage_anthropic(v),
    })
}

fn parse_usage_anthropic(v: &Value) -> Usage {
    let u = v.get("usage");
    Usage {
        input_tokens: u
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
        output_tokens: u
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
    }
}

// --- OpenAI ---

/// Buduje ciało żądania OpenAI Chat Completions (port `buildOpenAIRequest`).
pub fn build_openai_body(
    model: &str,
    system: &str,
    history: &[ChatMessage],
    variant: Variant,
) -> Value {
    let mut messages: Vec<Value> = vec![json!({"role": "system", "content": system})];
    for msg in history {
        if !msg.tool_results.is_empty() {
            for r in &msg.tool_results {
                messages.push(json!({
                    "role": "tool",
                    "content": r.content,
                    "tool_call_id": r.tool_use_id,
                }));
            }
        } else if !msg.tool_calls.is_empty() {
            let calls: Vec<Value> = msg
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": {
                            "name": c.name,
                            "arguments": Value::Object(c.arguments.clone()).to_string(),
                        }
                    })
                })
                .collect();
            let mut m = json!({"role": "assistant", "tool_calls": calls});
            if !msg.content.is_empty() {
                m["content"] = json!(msg.content);
            }
            messages.push(m);
        } else {
            messages.push(json!({"role": msg.role.as_str(), "content": msg.content}));
        }
    }

    json!({
        "model": model,
        "messages": messages,
        "tools": tools_json(variant, false),
        "temperature": 0.2,
    })
}

/// Parsuje odpowiedź OpenAI Chat Completions (port `requestOpenAI` dekodowania).
pub fn parse_openai_response(v: &Value) -> Result<LlmResponse, WireError> {
    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or(WireError::Empty)?;
    let message = choice
        .get("message")
        .ok_or_else(|| WireError::Malformed("brak message".into()))?;

    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
        for call in calls {
            let id = call.get("id").and_then(|i| i.as_str()).unwrap_or_default();
            let func = call.get("function");
            let name = func
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            // Argumenty OpenAI to string z zakodowanym JSON.
            let arguments: Map<String, Value> = func
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            tool_calls.push(ToolCall {
                id: id.to_string(),
                name: name.to_string(),
                arguments,
            });
        }
    }

    Ok(LlmResponse {
        text,
        tool_calls,
        usage: parse_usage_openai(v),
    })
}

fn parse_usage_openai(v: &Value) -> Usage {
    let u = v.get("usage");
    Usage {
        input_tokens: u
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
        output_tokens: u
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0),
    }
}

/// Buduje ciało + parsuje odpowiedź wg dostawcy (fasada dla transportu).
pub fn build_body(
    provider: crate::config::Provider,
    model: &str,
    system: &str,
    history: &[ChatMessage],
    variant: Variant,
) -> Value {
    match provider {
        crate::config::Provider::Anthropic => build_anthropic_body(model, system, history, variant),
        crate::config::Provider::OpenAiCompatible => {
            build_openai_body(model, system, history, variant)
        }
    }
}

/// Parsuje odpowiedź wg dostawcy.
pub fn parse_response(
    provider: crate::config::Provider,
    v: &Value,
) -> Result<LlmResponse, WireError> {
    match provider {
        crate::config::Provider::Anthropic => parse_anthropic_response(v),
        crate::config::Provider::OpenAiCompatible => parse_openai_response(v),
    }
}

// Pomocnik dla Role w JSON.
impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_body_includes_tools_and_history() {
        let history = vec![ChatMessage::user("Ustaw gain 50")];
        let body = build_anthropic_body("claude", "Jesteś asystentem", &history, Variant::Library);
        assert_eq!(body["model"], "claude");
        assert_eq!(body["system"], "Jesteś asystentem");
        assert!(body["tools"].as_array().unwrap().len() > 10);
        assert_eq!(body["messages"][0]["role"], "user");
    }

    #[test]
    fn anthropic_tool_results_become_user_blocks() {
        let msg = ChatMessage {
            role: Role::User,
            content: String::new(),
            tool_calls: vec![],
            tool_results: vec![crate::types::ToolResult {
                tool_use_id: "t1".into(),
                content: "ok".into(),
                is_error: false,
            }],
        };
        let body = build_anthropic_body("m", "s", &[msg], Variant::Library);
        let block = &body["messages"][0]["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "t1");
        assert!(block.get("is_error").is_none()); // brak przy sukcesie
    }

    #[test]
    fn parse_anthropic_text_and_tool_use() {
        let v = json!({
            "content": [
                {"type": "text", "text": "Robię to"},
                {"type": "tool_use", "id": "tu1", "name": "set_bpm",
                 "input": {"patchID": "p", "expectedRevision": 1, "bpm": 120}}
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let r = parse_anthropic_response(&v).unwrap();
        assert_eq!(r.text.as_deref(), Some("Robię to"));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "set_bpm");
        assert_eq!(r.tool_calls[0].arguments["bpm"], 120);
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 5);
    }

    #[test]
    fn parse_openai_decodes_stringified_arguments() {
        let v = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "set_name", "arguments": "{\"patchID\":\"p\",\"expectedRevision\":2,\"name\":\"Lead\"}"}
                    }]
                }
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 8}
        });
        let r = parse_openai_response(&v).unwrap();
        assert!(r.text.is_none());
        assert_eq!(r.tool_calls[0].name, "set_name");
        assert_eq!(r.tool_calls[0].arguments["name"], "Lead");
        assert_eq!(r.usage.input_tokens, 20);
    }

    #[test]
    fn parse_openai_empty_choices_errors() {
        let v = json!({"choices": []});
        assert_eq!(parse_openai_response(&v), Err(WireError::Empty));
    }

    #[test]
    fn openai_tool_call_roundtrips_through_history() {
        // Wiadomość asystenta z wywołaniem narzędzia serializuje argumenty jako string.
        let call = ToolCall {
            id: "c1".into(),
            name: "set_bpm".into(),
            arguments: serde_json::from_value(json!({"bpm": 120})).unwrap(),
        };
        let msg = ChatMessage {
            role: Role::Assistant,
            content: "ok".into(),
            tool_calls: vec![call],
            tool_results: vec![],
        };
        let body = build_openai_body("m", "s", &[msg], Variant::Library);
        let tc = &body["messages"][1]["tool_calls"][0];
        assert_eq!(tc["function"]["name"], "set_bpm");
        assert!(tc["function"]["arguments"]
            .as_str()
            .unwrap()
            .contains("120"));
    }
}

//! Transport HTTP dla modeli LLM (natywny; port `requestAnthropic`/`requestOpenAI`).
//!
//! Implementuje [`LlmClient`] przez `reqwest` blocking: buduje ciało przez [`wire`],
//! wysyła z nagłówkami zależnymi od dostawcy, parsuje odpowiedź. Format żądań i
//! parsowanie są czyste ([`wire`]) i testowane osobno — tu tylko wysyłka.
//!
//! Wyłączony na wasm (`cfg`), gdzie transportem jest fetch przeglądarki.

use crate::config::{AgentConfig, Provider};
use crate::run::{AgentError, LlmClient};
use crate::types::{ChatMessage, LlmResponse};
use crate::wire;
use mg101_commands::Variant;

/// Klient HTTP dla skonfigurowanego dostawcy. `system` i `variant` (zestaw
/// narzędzi) są stałe przez sesję.
pub struct HttpLlmClient {
    config: AgentConfig,
    system: String,
    variant: Variant,
    http: reqwest::blocking::Client,
}

impl HttpLlmClient {
    /// Tworzy klienta (konfiguracja normalizowana — endpoint/klucz/model).
    pub fn new(config: AgentConfig, system: impl Into<String>, variant: Variant) -> Self {
        // Hojny timeout żądania: odpowiedzi LLM (bez streamingu — dług #5) potrafią
        // przekroczyć domyślne 30 s reqwest i zerwać przebieg (review E7-parytet).
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .unwrap_or_default();
        Self {
            config: config.normalized(),
            system: system.into(),
            variant,
            http,
        }
    }
}

impl LlmClient for HttpLlmClient {
    fn complete(&self, history: &[ChatMessage]) -> Result<LlmResponse, AgentError> {
        let body = wire::build_body(
            self.config.provider,
            &self.config.model,
            &self.system,
            history,
            self.variant,
        );

        let mut req = self
            .http
            .post(&self.config.endpoint)
            .header("content-type", "application/json");
        req = match self.config.provider {
            Provider::Anthropic => req
                .header("x-api-key", &self.config.api_key)
                .header("anthropic-version", "2023-06-01"),
            Provider::OpenAiCompatible => {
                if self.config.api_key.is_empty() {
                    req
                } else {
                    req.header("authorization", format!("Bearer {}", self.config.api_key))
                }
            }
        };

        let resp = req
            .json(&body)
            .send()
            .map_err(|e| AgentError::Provider(format!("wysyłka: {e}")))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| AgentError::Provider(format!("odczyt treści: {e}")))?;
        if !status.is_success() {
            return Err(AgentError::Provider(format!(
                "HTTP {}: {}",
                status.as_u16(),
                text
            )));
        }

        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| AgentError::Provider(format!("JSON odpowiedzi: {e}")))?;
        wire::parse_response(self.config.provider, &value)
            .map_err(|e| AgentError::Provider(e.to_string()))
    }
}

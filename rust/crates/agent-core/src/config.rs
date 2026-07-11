//! Konfiguracja agenta i normalizacja endpointu — port `AgentConfiguration`
//! i `normalizeEndpoint`/`isConfigured` z v1. Czyste, testowalne.

use serde::{Deserialize, Serialize};

/// Dostawca LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Anthropic,
    /// Dowolny endpoint zgodny z OpenAI (`/chat/completions`).
    OpenAiCompatible,
}

/// Konfiguracja połączenia z modelem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub provider: Provider,
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
}

impl AgentConfig {
    /// Normalizuje pola (trim) i uzupełnia ścieżkę endpointu wg dostawcy.
    pub fn normalized(mut self) -> Self {
        self.endpoint = normalize_endpoint(self.endpoint.trim(), self.provider);
        self.model = self.model.trim().to_string();
        self.api_key = self.api_key.trim().to_string();
        self
    }

    /// Czy konfiguracja jest kompletna do wywołania (port `isConfigured`).
    pub fn is_configured(&self) -> bool {
        let e = self.endpoint.to_lowercase();
        let scheme_ok = e.starts_with("http://") || e.starts_with("https://");
        if !scheme_ok || self.model.is_empty() {
            return false;
        }
        self.provider != Provider::Anthropic || !self.api_key.is_empty()
    }
}

/// Uzupełnia schemat i ścieżkę endpointu wg dostawcy (port `normalizeEndpoint`).
/// Anthropic → `.../v1/messages`; OpenAI → `.../v1/chat/completions`.
pub fn normalize_endpoint(endpoint: &str, provider: Provider) -> String {
    let mut trimmed = endpoint.trim().to_string();
    if trimmed.is_empty() {
        return trimmed;
    }
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        trimmed = format!("http://{trimmed}");
    }
    // Rozdziel ścieżkę od (schematu+hosta) bez parsera URL — wystarczy analiza sufiksu.
    let (prefix, path) = split_path(&trimmed);
    let path_l = path.to_lowercase();

    match provider {
        Provider::Anthropic => {
            if path_l.ends_with("/messages") {
                trimmed
            } else if path.is_empty() || path == "/" {
                format!("{prefix}/v1/messages")
            } else {
                format!("{}/messages", trim_trailing_slash(&trimmed))
            }
        }
        Provider::OpenAiCompatible => {
            if path_l.ends_with("/chat/completions") {
                trimmed
            } else if path.is_empty() || path == "/" {
                format!("{prefix}/v1/chat/completions")
            } else if path_l.ends_with("/v1") {
                format!("{}/chat/completions", trim_trailing_slash(&trimmed))
            } else {
                format!("{}/v1/chat/completions", trim_trailing_slash(&trimmed))
            }
        }
    }
}

/// Zwraca (schemat+host, ścieżka) dla URL-a http(s).
fn split_path(url: &str) -> (String, String) {
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    match url[after_scheme..].find('/') {
        Some(rel) => {
            let abs = after_scheme + rel;
            (url[..abs].to_string(), url[abs..].to_string())
        }
        None => (url.to_string(), String::new()),
    }
}

fn trim_trailing_slash(s: &str) -> &str {
    s.strip_suffix('/').unwrap_or(s)
}

/// Normalizuje leksykalnie ścieżkę ABSOLUTNĄ (usuwa `.`/`..`/podwójne `/`).
/// Zwraca `None` dla ścieżek względnych — te nigdy nie są w sandboxie (parytet z
/// v1 `standardized.path`, ale odrzucamy względne zamiast kotwiczyć do CWD).
/// `..` nie może wyjść ponad korzeń.
pub fn normalize_lexical(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    Some(format!("/{}", stack.join("/")))
}

/// Czy `path` mieści się w którymś z zatwierdzonych katalogów (port
/// `isPathApproved`). Obie strony są normalizowane leksykalnie; ścieżki względne
/// i puste korzenie NIGDY nie pasują (domknięcie luki traversal z review E6/K1).
pub fn is_path_approved(path: &str, approved_roots: &[String]) -> bool {
    let Some(p) = normalize_lexical(path) else {
        return false;
    };
    approved_roots.iter().any(|root| {
        match normalize_lexical(root) {
            // Pusty/„/” korzeń nie zatwierdza wszystkiego automatycznie: wymagamy
            // realnego prefiksu katalogu.
            Some(r) if r != "/" => p == r || p.starts_with(&format!("{r}/")),
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_endpoint_normalization() {
        assert_eq!(
            normalize_endpoint("https://api.anthropic.com", Provider::Anthropic),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            normalize_endpoint("https://proxy.local/api", Provider::Anthropic),
            "https://proxy.local/api/messages"
        );
        // Już znormalizowany — bez zmian.
        assert_eq!(
            normalize_endpoint("https://x/v1/messages", Provider::Anthropic),
            "https://x/v1/messages"
        );
        // Bez schematu → http://.
        assert_eq!(
            normalize_endpoint("localhost:8080", Provider::Anthropic),
            "http://localhost:8080/v1/messages"
        );
    }

    #[test]
    fn openai_endpoint_normalization() {
        assert_eq!(
            normalize_endpoint("https://api.openai.com", Provider::OpenAiCompatible),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://host/v1", Provider::OpenAiCompatible),
            "https://host/v1/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://host/custom", Provider::OpenAiCompatible),
            "https://host/custom/v1/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("https://x/v1/chat/completions", Provider::OpenAiCompatible),
            "https://x/v1/chat/completions"
        );
    }

    #[test]
    fn is_configured_requires_key_only_for_anthropic() {
        let anthropic = AgentConfig {
            provider: Provider::Anthropic,
            endpoint: "https://x/v1/messages".into(),
            model: "claude".into(),
            api_key: String::new(),
        };
        assert!(!anthropic.is_configured()); // brak klucza
        assert!(AgentConfig {
            api_key: "k".into(),
            ..anthropic.clone()
        }
        .is_configured());

        let openai = AgentConfig {
            provider: Provider::OpenAiCompatible,
            endpoint: "http://localhost:1234/v1/chat/completions".into(),
            model: "local".into(),
            api_key: String::new(),
        };
        assert!(openai.is_configured()); // klucz opcjonalny
    }

    #[test]
    fn path_approval_matches_subdirs_only() {
        let roots = vec!["/home/user/patches".to_string()];
        assert!(is_path_approved("/home/user/patches/a.mg101patch", &roots));
        assert!(is_path_approved("/home/user/patches", &roots));
        assert!(!is_path_approved("/home/user/patches-evil", &roots));
        assert!(!is_path_approved("/etc/passwd", &roots));
    }

    #[test]
    fn path_traversal_and_relative_are_rejected() {
        // Regresja K1 (review E6): normalizacja leksykalna blokuje `..` i ścieżki
        // względne; pusty/„/” korzeń nie zatwierdza wszystkiego.
        let roots = vec!["/data".to_string()];
        assert!(!is_path_approved("/data/../etc/passwd", &roots));
        assert!(is_path_approved("/data/./sub/x", &roots));
        assert!(!is_path_approved("relative/x", &roots)); // względna
                                                          // Pusty korzeń (z buga parent_dir) NIE zatwierdza wszystkiego.
        assert!(!is_path_approved("/etc/passwd", &["".to_string()]));
        assert!(!is_path_approved("/etc/passwd", &["/".to_string()]));
    }

    #[test]
    fn normalize_lexical_resolves_dots() {
        assert_eq!(normalize_lexical("/a/b/../c").as_deref(), Some("/a/c"));
        assert_eq!(normalize_lexical("/a/./b//c").as_deref(), Some("/a/b/c"));
        assert_eq!(normalize_lexical("/../../x").as_deref(), Some("/x")); // nie ucieka ponad /
        assert!(normalize_lexical("relative").is_none());
    }
}

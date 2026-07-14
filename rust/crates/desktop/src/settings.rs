//! Trwałe ustawienia aplikacji (`settings.json` w katalogu danych).
//!
//! Dotąd nic poza kluczem API (Keychain) nie przeżywało restartu — dostawca,
//! endpoint, model i język wracały do domyślnych przy każdym starcie. Tutaj są
//! zapisywane jawnie.
//!
//! Klucza API **nigdy** tu nie ma — on należy do systemowego magazynu sekretów
//! ([`crate::keychain`]). Plik ustawień bywa kopiowany/wersjonowany, sekret nie może
//! w nim wylądować.

use serde::{Deserialize, Serialize};

/// Sposób rozmowy z modelem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// Anthropic Messages API — wymaga klucza API i kredytów.
    #[default]
    Anthropic,
    /// Dowolny endpoint zgodny z OpenAI (także lokalny: Ollama, LM Studio).
    OpenAiCompatible,
    /// Lokalnie zainstalowany Claude Code (`claude -p`) — płaci **subskrypcja**,
    /// klucz API niepotrzebny. Narzędzia dostaje przez mostek MCP do żywego Studio.
    ClaudeCode,
}

impl Backend {
    pub fn as_index(self) -> i32 {
        match self {
            Backend::Anthropic => 0,
            Backend::OpenAiCompatible => 1,
            Backend::ClaudeCode => 2,
        }
    }

    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Backend::OpenAiCompatible,
            2 => Backend::ClaudeCode,
            _ => Backend::Anthropic,
        }
    }

    /// Czy ten backend wymaga klucza API (a więc i odczytu z Keychain).
    pub fn needs_api_key(self) -> bool {
        matches!(self, Backend::Anthropic)
    }
}

fn default_endpoint() -> String {
    "https://api.anthropic.com".into()
}
fn default_model() -> String {
    "claude-sonnet-5".into()
}
fn default_lang() -> String {
    "en".into()
}
fn default_claude_bin() -> String {
    "claude".into()
}
fn default_true() -> bool {
    true
}

/// Ustawienia zapisywane na dysk. Każde pole ma `default`, więc plik z poprzedniej
/// wersji (bez nowych kluczy) wczytuje się bez migracji.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub backend: Backend,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_lang")]
    pub lang: String,
    /// Mostek MCP → żywe Studio. Domyślnie WŁĄCZONY: bez niego backend
    /// `ClaudeCode` nie ma czym edytować Biblioteki. Nasłuch tylko na 127.0.0.1,
    /// za tokenem — patrz [`crate::bridge`].
    #[serde(default = "default_true")]
    pub bridge_enabled: bool,
    /// Ścieżka do binarki Claude Code (gdy nie ma jej w `PATH`).
    #[serde(default = "default_claude_bin")]
    pub claude_bin: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            endpoint: default_endpoint(),
            model: default_model(),
            lang: default_lang(),
            bridge_enabled: true,
            claude_bin: default_claude_bin(),
        }
    }
}

impl Settings {
    fn path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("settings.json")
    }

    /// Wczytuje ustawienia; brak pliku lub uszkodzony JSON → domyślne (nie błąd:
    /// aplikacja musi wstać nawet z zepsutą konfiguracją).
    pub fn load(dir: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(Self::path(dir)) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("settings.json niepoprawny ({e}) — używam domyślnych.");
            Self::default()
        })
    }

    /// Zapisuje ustawienia. Zwraca komunikat błędu przy niepowodzeniu.
    pub fn save(&self, dir: &std::path::Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(dir), text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Katalog unikalny per test — testy biegną RÓWNOLEGLE, więc wspólny katalog
    /// (po samym PID) powodował, że jeden test kasował plik drugiemu.
    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mg101-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = tmp("roundtrip");
        let s = Settings {
            backend: Backend::ClaudeCode,
            endpoint: "https://x".into(),
            model: "m".into(),
            lang: "pl".into(),
            bridge_enabled: false,
            claude_bin: "/usr/local/bin/claude".into(),
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_gives_defaults_not_error() {
        let dir = tmp("defaults");
        let s = Settings::load(&dir);
        assert_eq!(s.backend, Backend::Anthropic);
        assert!(s.bridge_enabled, "mostek domyślnie włączony");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_file_without_new_keys_still_loads() {
        // Regresja: dokładanie pól nie może psuć istniejącej konfiguracji.
        let dir = tmp("oldfile");
        std::fs::write(
            dir.join("settings.json"),
            r#"{"backend":"open_ai_compatible","model":"llama3"}"#,
        )
        .unwrap();
        let s = Settings::load(&dir);
        assert_eq!(s.backend, Backend::OpenAiCompatible);
        assert_eq!(s.model, "llama3");
        assert_eq!(s.endpoint, default_endpoint(), "brakujące pole → domyślne");
        assert!(s.bridge_enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tmp("corrupt");
        std::fs::write(dir.join("settings.json"), "{to nie jest json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_anthropic_needs_an_api_key() {
        assert!(Backend::Anthropic.needs_api_key());
        assert!(!Backend::ClaudeCode.needs_api_key());
        assert!(!Backend::OpenAiCompatible.needs_api_key());
    }

    #[test]
    fn backend_index_roundtrips() {
        for b in [Backend::Anthropic, Backend::OpenAiCompatible, Backend::ClaudeCode] {
            assert_eq!(Backend::from_index(b.as_index()), b);
        }
    }
}

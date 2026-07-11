//! Katalog efektów — port `EffectCatalog` ze Swift. Klucze JSON snake_case
//! (odpowiadają nazwom pól Rust wprost, bez rename).

use crate::error::ProfileError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parametr modelu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub local_index: i64,
    pub file_offset: usize,
    pub raw_range: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_confidence: Option<String>,
    /// Pewność zapisu (offset/kodowanie). `Some("confirmed")` = potwierdzone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_confidence: Option<String>,
    /// Numer MIDI CC sterujący parametrem (jeśli znany).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_cc: Option<i64>,
    /// Jednostka fizyczna wartości (np. `dB`, `Hz`), jeśli znana.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Typy kontrolek UI wg QuickTone (2=suwak, 7=przełącznik 0/1, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui_element_types: Vec<i64>,
}

/// Typ kontrolki UI dla parametru — sterowany danymi katalogu (ADR-0002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Suwak/pokrętło o zakresie ciągłym.
    Slider,
    /// Przełącznik dwustanowy (0/1).
    Toggle,
}

impl Control {
    /// Nazwa stabilna do JSON/UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Control::Slider => "slider",
            Control::Toggle => "toggle",
        }
    }
}

/// Kod `ui_element_types` QuickTone dla przełącznika dwustanowego.
const UET_TOGGLE: i64 = 7;

impl Parameter {
    /// Dolny kraniec zakresu (domyślnie 0).
    pub fn minimum(&self) -> i64 {
        self.raw_range.first().copied().unwrap_or(0)
    }
    /// Górny kraniec zakresu (domyślnie 100).
    pub fn maximum(&self) -> i64 {
        self.raw_range.get(1).copied().unwrap_or(100)
    }

    /// Etykieta do UI — `display_name`, a w razie braku techniczna `name`.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// Kontrolka wg danych katalogu: przełącznik gdy UET=7 lub zakres 0..1.
    pub fn control(&self) -> Control {
        if self.ui_element_types.contains(&UET_TOGGLE)
            || (self.minimum() == 0 && self.maximum() == 1)
        {
            Control::Toggle
        } else {
            Control::Slider
        }
    }

    /// Czy semantyka i zapis są potwierdzone (nie „inferred/unknown").
    /// Brak pola pewności traktujemy jako potwierdzony (starsze wpisy).
    pub fn is_confirmed(&self) -> bool {
        let ok = |c: &Option<String>| c.as_deref().map(|s| s == "confirmed").unwrap_or(true);
        ok(&self.semantic_confidence) && ok(&self.storage_confidence)
    }
}

/// Model (typ efektu) w bloku.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub display_name: String,
    pub slug: String,
    pub model_id: i64,
    pub parameters: Vec<Parameter>,
}

/// Moduł = zbiór modeli danego bloku (klucz = string model_id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub models: BTreeMap<String, Model>,
}

/// Katalog efektów całego urządzenia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectCatalog {
    pub modules: BTreeMap<String, Module>,
}

impl EffectCatalog {
    /// Parsuje katalog z JSON.
    pub fn from_json(s: &str) -> Result<Self, ProfileError> {
        serde_json::from_str(s).map_err(|e| ProfileError::Malformed(e.to_string()))
    }

    /// Modele danego bloku, posortowane po `model_id`.
    pub fn models(&self, block: &str) -> Vec<&Model> {
        let Some(module) = self.modules.get(block) else {
            return Vec::new();
        };
        let mut v: Vec<&Model> = module.models.values().collect();
        v.sort_by_key(|m| m.model_id);
        v
    }

    /// Model po bloku i id.
    pub fn model(&self, block: &str, id: i64) -> Option<&Model> {
        self.modules.get(block)?.models.get(&id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(json: &str) -> Parameter {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn label_falls_back_to_name() {
        let p = param(r#"{"name":"gain","local_index":0,"file_offset":32,"raw_range":[0,100]}"#);
        assert_eq!(p.label(), "gain");
        let p = param(
            r#"{"name":"gain","local_index":0,"file_offset":32,"raw_range":[0,100],"display_name":"GAIN"}"#,
        );
        assert_eq!(p.label(), "GAIN");
    }

    #[test]
    fn control_is_toggle_for_uet7_or_binary_range() {
        let by_uet = param(
            r#"{"name":"bright","local_index":1,"file_offset":33,"raw_range":[0,100],"ui_element_types":[7]}"#,
        );
        assert_eq!(by_uet.control(), Control::Toggle);
        let by_range = param(r#"{"name":"on","local_index":0,"file_offset":1,"raw_range":[0,1]}"#);
        assert_eq!(by_range.control(), Control::Toggle);
        let slider =
            param(r#"{"name":"level","local_index":2,"file_offset":34,"raw_range":[0,100]}"#);
        assert_eq!(slider.control(), Control::Slider);
    }

    #[test]
    fn confirmed_requires_both_confidences() {
        // Brak pól → traktowane jako potwierdzone (starsze wpisy).
        let bare = param(r#"{"name":"x","local_index":0,"file_offset":1,"raw_range":[0,1]}"#);
        assert!(bare.is_confirmed());
        let confirmed = param(
            r#"{"name":"x","local_index":0,"file_offset":1,"raw_range":[0,1],"semantic_confidence":"confirmed","storage_confidence":"confirmed"}"#,
        );
        assert!(confirmed.is_confirmed());
        let inferred = param(
            r#"{"name":"x","local_index":0,"file_offset":1,"raw_range":[0,1],"semantic_confidence":"inferred","storage_confidence":"confirmed"}"#,
        );
        assert!(!inferred.is_confirmed());
    }
}

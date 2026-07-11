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
}

impl Parameter {
    /// Dolny kraniec zakresu (domyślnie 0).
    pub fn minimum(&self) -> i64 {
        self.raw_range.first().copied().unwrap_or(0)
    }
    /// Górny kraniec zakresu (domyślnie 100).
    pub fn maximum(&self) -> i64 {
        self.raw_range.get(1).copied().unwrap_or(100)
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

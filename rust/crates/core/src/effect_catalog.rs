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
    /// Wzór przeliczenia wartości surowej (0..100) na fizyczną (dB/Hz), jeśli znany.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_transform: Option<String>,
    /// Parametr wyliczeniowy (enum): skończony zbiór stanów o WŁASNYCH wartościach
    /// surowych (niekoniecznie 0/1). Np. POSITION = PRECEDE(1)/POSTERIOR(128).
    /// Pusty = parametr ciągły/logiczny wg `raw_range`/`ui_element_types`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<ParamValue>,
}

/// Jeden stan parametru wyliczeniowego: surowa wartość + etykieta do UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamValue {
    pub value: i64,
    pub label: String,
}

/// Typ kontrolki UI dla parametru — sterowany danymi katalogu (ADR-0002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Suwak/pokrętło o zakresie ciągłym.
    Slider,
    /// Przełącznik dwustanowy (0/1).
    Toggle,
    /// Wyliczenie: skończony zbiór stanów o własnych wartościach (patrz `values`).
    Enum,
}

impl Control {
    /// Nazwa stabilna do JSON/UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Control::Slider => "slider",
            Control::Toggle => "toggle",
            Control::Enum => "enum",
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

    /// Kontrolka wg danych katalogu: enum gdy zdefiniowano `values`, przełącznik
    /// gdy UET=7 lub zakres 0..1, inaczej suwak.
    pub fn control(&self) -> Control {
        if !self.values.is_empty() {
            Control::Enum
        } else if self.ui_element_types.contains(&UET_TOGGLE)
            || (self.minimum() == 0 && self.maximum() == 1)
        {
            Control::Toggle
        } else {
            Control::Slider
        }
    }

    /// Etykieta stanu enuma dla wartości surowej (np. 1→"PRECEDE"). Nieznana
    /// wartość → sama liczba (bezpieczne dla nietypowych bajtów).
    pub fn enum_label(&self, raw: i64) -> String {
        self.values
            .iter()
            .find(|v| v.value == raw)
            .map(|v| v.label.clone())
            .unwrap_or_else(|| raw.to_string())
    }

    /// Następny stan enuma (cykl) względem wartości surowej — do przełącznika.
    /// Nieznana bieżąca wartość → pierwszy zdefiniowany stan.
    pub fn enum_next(&self, raw: i64) -> i64 {
        if self.values.is_empty() {
            return raw;
        }
        match self.values.iter().position(|v| v.value == raw) {
            Some(i) => self.values[(i + 1) % self.values.len()].value,
            None => self.values[0].value,
        }
    }

    /// Etykiety wszystkich stanów enuma (model dropdownu).
    pub fn enum_labels(&self) -> Vec<String> {
        self.values.iter().map(|v| v.label.clone()).collect()
    }

    /// Wartości surowe wszystkich stanów enuma (równoległe do `enum_labels`).
    pub fn enum_values(&self) -> Vec<i64> {
        self.values.iter().map(|v| v.value).collect()
    }

    /// Indeks bieżącego stanu enuma w liście (do dropdownu); −1 gdy nieznany.
    pub fn enum_index(&self, raw: i64) -> i64 {
        self.values
            .iter()
            .position(|v| v.value == raw)
            .map(|i| i as i64)
            .unwrap_or(-1)
    }

    /// Czy semantyka i zapis są potwierdzone (nie „inferred/unknown").
    /// Brak pola pewności traktujemy jako potwierdzony (starsze wpisy).
    pub fn is_confirmed(&self) -> bool {
        let ok = |c: &Option<String>| c.as_deref().map(|s| s == "confirmed").unwrap_or(true);
        ok(&self.semantic_confidence) && ok(&self.storage_confidence)
    }

    /// Wartość fizyczna dla `raw` (0..100) wg `display_transform`, sformatowana
    /// z jednostką (np. `-3.6 dB`, `245 Hz`). `None`, gdy brak/nieznany wzór.
    ///
    /// Wzory potwierdzone z QuickTone (`cabinet_display_tables`): dB liniowe,
    /// low/high-cut wykładnicze. Rozpoznawane po zawartości `display_transform`.
    pub fn display_value(&self, raw: i64) -> Option<String> {
        let t = self.display_transform.as_deref()?;
        let r = raw as f64 / 100.0;
        if t.contains("low_cut_table") {
            Some(format!("{:.0} Hz", 20.0 * 50f64.powf(r)))
        } else if t.contains("high_cut_table") {
            Some(format!("{:.0} Hz", 5000.0 * 4f64.powf(r)))
        } else if t == "raw / 100 * 24 - 12" {
            Some(format!("{:+.1} dB", raw as f64 / 100.0 * 24.0 - 12.0))
        } else {
            None
        }
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
    fn display_value_computes_physical_units() {
        let db = param(
            r#"{"name":"level","local_index":0,"file_offset":80,"raw_range":[0,100],"unit":"dB","display_transform":"raw / 100 * 24 - 12"}"#,
        );
        assert_eq!(db.display_value(0).as_deref(), Some("-12.0 dB"));
        assert_eq!(db.display_value(50).as_deref(), Some("+0.0 dB"));
        assert_eq!(db.display_value(100).as_deref(), Some("+12.0 dB"));
        let low = param(
            r#"{"name":"low_cut","local_index":1,"file_offset":81,"raw_range":[0,100],"unit":"Hz","display_transform":"quicktone_low_cut_table[raw]"}"#,
        );
        assert_eq!(low.display_value(0).as_deref(), Some("20 Hz")); // endpoint dolny
        assert_eq!(low.display_value(100).as_deref(), Some("1000 Hz")); // endpoint górny
                                                                        // Brak wzoru → brak wartości fizycznej.
        let plain =
            param(r#"{"name":"gain","local_index":0,"file_offset":32,"raw_range":[0,100]}"#);
        assert!(plain.display_value(50).is_none());
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

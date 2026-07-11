//! Rozliczanie kosztu tokenów. Cennik jako **dane** (nie zaszyty w kodzie) —
//! nowy model/dostawca to nowy wpis, nie zmiana logiki.

use crate::types::Usage;

/// Cennik modelu w USD za milion tokenów (wej./wyj.).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

impl ModelPricing {
    /// Koszt w USD dla danego zużycia.
    pub fn cost_usd(&self, usage: Usage) -> f64 {
        (usage.input_tokens as f64 / 1_000_000.0) * self.input_per_mtok
            + (usage.output_tokens as f64 / 1_000_000.0) * self.output_per_mtok
    }
}

/// Cennik po prefiksie nazwy modelu (dopasowanie po najdłuższym prefiksie).
/// Zwraca `None`, gdy model nieznany — wtedy koszt nie jest raportowany.
pub fn pricing_for(model: &str, table: &[(&str, ModelPricing)]) -> Option<ModelPricing> {
    table
        .iter()
        .filter(|(prefix, _)| model.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, p)| *p)
}

/// Domyślny cennik referencyjny (aktualizowalny; dane, nie logika).
pub fn default_pricing() -> Vec<(&'static str, ModelPricing)> {
    vec![
        (
            "claude-opus",
            ModelPricing {
                input_per_mtok: 15.0,
                output_per_mtok: 75.0,
            },
        ),
        (
            "claude-sonnet",
            ModelPricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
            },
        ),
        (
            "claude-haiku",
            ModelPricing {
                input_per_mtok: 0.8,
                output_per_mtok: 4.0,
            },
        ),
        (
            "gpt-4o",
            ModelPricing {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_computed_from_usage() {
        let p = ModelPricing {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        };
        let cost = p.cost_usd(Usage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
        });
        assert!((cost - 18.0).abs() < 1e-9);
    }

    #[test]
    fn pricing_matches_longest_prefix() {
        let table = default_pricing();
        let p = pricing_for("claude-opus-4-8", &table).unwrap();
        assert_eq!(p.input_per_mtok, 15.0);
        assert!(pricing_for("unknown-model", &table).is_none());
    }
}

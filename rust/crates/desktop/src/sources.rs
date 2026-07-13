//! Katalog publicznych źródeł patchy MG-101 — **odnośniki, nie pliki**.
//!
//! Dlaczego tylko odnośniki: żadne znane publiczne źródło patchy MG-101 (ChopTones,
//! paczki autorskie, wątki forumowe) nie udziela licencji na redystrybucję.
//! „Darmowe do pobrania" nie znaczy „wolno zapakować w cudzą aplikację" — dołączenie
//! tych plików do repo/instalatora byłoby rozpowszechnianiem cudzej własności bez zgody.
//!
//! Zamiast tego: aplikacja pokazuje, GDZIE patche zdobyć i CZYJE są, a po imporcie
//! przypisuje im autora, źródło i warunki użycia ([`mg101_library::PatchMeta`]).

use serde::Deserialize;

/// Osadzony katalog (dane, nie kod — kuratorowany ręcznie).
const CATALOG_JSON: &str = include_str!("../assets/patch-sources.json");

/// Jedno źródło patchy: kto, gdzie, na jakich warunkach.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PatchSource {
    pub id: String,
    pub name: String,
    pub author: String,
    pub url: String,
    /// Warunki użycia — celowo tekst, nie enum: realne warunki bywają nieostre
    /// („darmowe, brak wyraźnej licencji"), a udawanie precyzji wprowadzałoby w błąd.
    pub license: String,
    /// Czy źródło jest płatne (komercyjne).
    #[serde(default)]
    pub paid: bool,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    sources: Vec<PatchSource>,
}

/// Zwraca katalog źródeł. Parsuje osadzony JSON; przy błędzie danych zwraca pustą
/// listę (brak katalogu nie może wywrócić aplikacji — to funkcja pomocnicza).
pub fn all() -> Vec<PatchSource> {
    match serde_json::from_str::<Catalog>(CATALOG_JSON) {
        Ok(c) => c.sources,
        Err(e) => {
            eprintln!("Katalog źródeł patchy niepoprawny: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_is_not_empty() {
        let sources = all();
        assert!(!sources.is_empty(), "katalog źródeł nie może być pusty");
    }

    #[test]
    fn every_source_is_complete_and_has_https_url() {
        for s in all() {
            assert!(!s.id.trim().is_empty(), "puste id");
            assert!(!s.name.trim().is_empty(), "{}: pusta nazwa", s.id);
            // Autor i licencja są SENSEM tego katalogu — wpis bez nich nie niesie
            // informacji o tym, czyj jest patch i co wolno z nim zrobić.
            assert!(!s.author.trim().is_empty(), "{}: brak autora", s.id);
            assert!(!s.license.trim().is_empty(), "{}: brak licencji", s.id);
            assert!(
                s.url.starts_with("https://"),
                "{}: URL musi być https ({})",
                s.id,
                s.url
            );
        }
    }

    #[test]
    fn source_ids_are_unique() {
        let sources = all();
        let mut ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "zduplikowane id w katalogu źródeł");
    }
}

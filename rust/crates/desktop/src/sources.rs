//! Katalog publicznych źródeł patchy MG-101 — **odnośniki, nie pliki**.
//!
//! Dlaczego tylko odnośniki: żadne znane publiczne źródło patchy MG-101 (ChopTones,
//! paczki autorskie, wątki forumowe) nie udziela licencji na redystrybucję.
//! „Darmowe do pobrania" nie znaczy „wolno zapakować w cudzą aplikację" — dołączenie
//! tych plików do repo/instalatora byłoby rozpowszechnianiem cudzej własności bez zgody.
//!
//! Zamiast tego: aplikacja pokazuje, GDZIE patche zdobyć i CZYJE są, a po imporcie
//! przypisuje im autora, źródło i warunki użycia ([`mg101_library::PatchMeta`]).

use crate::Lang;
use serde::Deserialize;

/// Osadzony katalog (dane, nie kod — kuratorowany ręcznie).
const CATALOG_JSON: &str = include_str!("../assets/patch-sources.json");

/// Surowy wpis katalogu — opis i licencja w obu językach.
#[derive(Debug, Clone, Deserialize)]
struct RawSource {
    id: String,
    name: String,
    author: String,
    url: String,
    #[serde(default)]
    paid: bool,
    license_pl: String,
    license_en: String,
    description_pl: String,
    description_en: String,
}

/// Jedno źródło patchy: kto, gdzie, na jakich warunkach — już w wybranym języku.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchSource {
    pub id: String,
    /// Nazwa własna paczki — NIE tłumaczona.
    pub name: String,
    /// Autor (osoba/marka) — NIE tłumaczony.
    pub author: String,
    pub url: String,
    /// Warunki użycia — celowo tekst, nie enum: realne warunki bywają nieostre
    /// („darmowe, brak wyraźnej licencji"), a udawanie precyzji wprowadzałoby w błąd.
    pub license: String,
    /// Czy źródło jest płatne (komercyjne).
    pub paid: bool,
    pub description: String,
}

#[derive(Debug, Deserialize)]
struct Catalog {
    sources: Vec<RawSource>,
}

/// Zwraca katalog źródeł w danym języku. Parsuje osadzony JSON; przy błędzie danych
/// zwraca pustą listę (brak katalogu nie może wywrócić aplikacji — to funkcja pomocnicza).
pub fn all(lang: Lang) -> Vec<PatchSource> {
    match serde_json::from_str::<Catalog>(CATALOG_JSON) {
        Ok(c) => c
            .sources
            .into_iter()
            .map(|s| PatchSource {
                license: match lang {
                    Lang::Pl => s.license_pl,
                    Lang::En => s.license_en,
                },
                description: match lang {
                    Lang::Pl => s.description_pl,
                    Lang::En => s.description_en,
                },
                id: s.id,
                name: s.name,
                author: s.author,
                url: s.url,
                paid: s.paid,
            })
            .collect(),
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
        assert!(!all(Lang::Pl).is_empty(), "katalog źródeł nie może być pusty");
        assert!(!all(Lang::En).is_empty());
    }

    #[test]
    fn both_languages_are_present_and_actually_differ() {
        // Sedno: zakładka Źródła ma być tłumaczona RAZEM z danymi, nie tylko etykietami.
        let pl = all(Lang::Pl);
        let en = all(Lang::En);
        assert_eq!(pl.len(), en.len());
        for (p, e) in pl.iter().zip(en.iter()) {
            assert_eq!(p.id, e.id);
            // Nazwa własna i autor się nie tłumaczą...
            assert_eq!(p.name, e.name);
            assert_eq!(p.author, e.author);
            // ...ale opis i licencja MUSZĄ (inaczej EN pokazywałby polski tekst).
            assert_ne!(p.description, e.description, "{}: opis nieprzetłumaczony", p.id);
            assert_ne!(p.license, e.license, "{}: licencja nieprzetłumaczona", p.id);
        }
    }

    #[test]
    fn every_source_is_complete_and_has_https_url() {
        for s in all(Lang::Pl) {
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
        let sources = all(Lang::Pl);
        let mut ids: Vec<&str> = sources.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "zduplikowane id w katalogu źródeł");
    }
}

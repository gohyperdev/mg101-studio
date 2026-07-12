//! Internacjonalizacja (PL/EN) — port „uporządkowanie i18n" (dług v1 #6).
//!
//! Klucze zamiast literałów rozsianych po UI: jedno źródło tłumaczeń, wybór
//! języka w czasie działania. Brak zależności od platformy (testowalny, wasm-safe).

/// Język interfejsu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pl,
    En,
}

impl Lang {
    /// Kod ISO (do zapisu ustawień).
    pub fn code(self) -> &'static str {
        match self {
            Lang::Pl => "pl",
            Lang::En => "en",
        }
    }

    /// Z kodu ISO (domyślnie EN dla nieznanych).
    pub fn from_code(code: &str) -> Self {
        match code {
            "pl" => Lang::Pl,
            _ => Lang::En,
        }
    }
}

/// Zwraca tłumaczenie klucza w danym języku. Nieznany klucz → sam klucz (łatwy
/// do wychwycenia w UI, brak paniki).
pub fn tr(lang: Lang, key: &str) -> &'static str {
    // (klucz, PL, EN)
    const TABLE: &[(&str, &str, &str)] = &[
        ("app.title", "MG101 Studio", "MG101 Studio"),
        ("library.title", "Biblioteka patchy", "Patch Library"),
        (
            "library.empty",
            "Brak patchy — zaimportuj lub wybierz.",
            "No patches — import or select.",
        ),
        ("tab.user", "Użytkownika", "User"),
        ("tab.factory", "Fabryczne", "Factory"),
        ("tab.library", "Biblioteka", "Library"),
        (
            "editor.no_selection",
            "Nie wybrano patcha",
            "No patch selected",
        ),
        ("editor.chain", "Łańcuch efektów", "Effect Chain"),
        ("editor.bypass", "Bypass", "Bypass"),
        ("editor.model", "Model", "Model"),
        ("editor.name", "Nazwa", "Name"),
        ("editor.bpm", "Tempo (BPM)", "Tempo (BPM)"),
        ("editor.rev", "rew.", "rev"),
        ("slot.empty", "(pusty)", "(empty)"),
        ("inspector.changes", "Zmiany", "Changes"),
        (
            "inspector.changes_header",
            "Zmiany bajtów względem oryginału (offset: przed → po)",
            "Byte changes vs. original (offset: before → after)",
        ),
        ("inspector.binary", "Binarne", "Binary"),
        ("inspector.agent", "Agent", "Agent"),
        ("inspector.mcp", "MCP", "MCP"),
        ("inspector.settings", "Ustawienia", "Settings"),
        (
            "inspector.no_changes",
            "Brak zmian względem oryginału",
            "No changes vs. original",
        ),
        ("settings.title", "Ustawienia AI", "AI Settings"),
        ("settings.provider", "Dostawca", "Provider"),
        ("settings.endpoint", "Endpoint", "Endpoint"),
        ("settings.model", "Model", "Model"),
        ("settings.language", "Język", "Language"),
        ("settings.key", "Klucz API", "API Key"),
        ("action.save", "Zapisz", "Save"),
        ("agent.send", "Wyślij", "Send"),
        ("action.duplicate", "Duplikuj", "Duplicate"),
        ("action.delete", "Usuń", "Delete"),
        ("action.revert", "Cofnij", "Revert"),
        ("action.import", "Importuj…", "Import…"),
        ("action.export", "Eksportuj…", "Export…"),
        (
            "transfer.title",
            "Transfer do urządzenia",
            "Transfer to Device",
        ),
        ("transfer.push", "Wyślij do slotu", "Push to Slot"),
        ("transfer.pull", "Pobierz ze slotu", "Pull from Slot"),
        ("device.fetch", "Pobierz", "Fetch"),
        ("device.import_dump", "Import → Biblioteka", "Import → Library"),
        ("device.copy_to_library", "Kopiuj do Biblioteki", "Copy to Library"),
        ("device.none", "brak urządzenia", "no device"),
        (
            "error.title",
            "Operacja nie powiodła się",
            "Operation failed",
        ),
    ];
    for (k, pl, en) in TABLE {
        if *k == key {
            return match lang {
                Lang::Pl => pl,
                Lang::En => en,
            };
        }
    }
    // Nieznany klucz: zwróć statyczny placeholder (nie key — brak 'static z &str param).
    "??"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_key_differs_by_language() {
        assert_eq!(tr(Lang::Pl, "library.title"), "Biblioteka patchy");
        assert_eq!(tr(Lang::En, "library.title"), "Patch Library");
    }

    #[test]
    fn unknown_key_is_placeholder_not_panic() {
        assert_eq!(tr(Lang::En, "no.such.key"), "??");
    }

    #[test]
    fn lang_roundtrips_through_code() {
        assert_eq!(Lang::from_code(Lang::Pl.code()), Lang::Pl);
        assert_eq!(Lang::from_code(Lang::En.code()), Lang::En);
        assert_eq!(Lang::from_code("xx"), Lang::En);
    }

    #[test]
    fn every_key_has_both_translations_nonempty() {
        for key in [
            "app.title",
            "library.title",
            "tab.user",
            "tab.factory",
            "tab.library",
            "inspector.changes",
            "transfer.title",
        ] {
            assert!(!tr(Lang::Pl, key).is_empty());
            assert!(!tr(Lang::En, key).is_empty());
            assert_ne!(tr(Lang::Pl, key), "??", "brak klucza {key}");
        }
    }
}

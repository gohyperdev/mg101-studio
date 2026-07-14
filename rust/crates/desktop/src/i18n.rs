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
            "tip.value",
            "Wpisz wartość (zakres obok) i zatwierdź Enter",
            "Type a value (range shown) and press Enter",
        ),
        ("tip.inc", "Zwiększ o 1", "Increase by 1"),
        ("tip.toggle", "Przełącz stan", "Toggle state"),
        ("tip.dec", "Zmniejsz o 1", "Decrease by 1"),
        ("tip.import", "Importuj patch z pliku .mg101patch", "Import a patch from a .mg101patch file"),
        (
            "tip.export",
            "Zapisz wybrany patch do pliku .mg101patch",
            "Save the selected patch to a .mg101patch file",
        ),
        (
            "tip.duplicate",
            "Utwórz kopię wybranego patcha w Bibliotece",
            "Create a copy of the selected patch in the Library",
        ),
        ("tip.delete", "Usuń wybrany patch z Biblioteki", "Delete the selected patch from the Library"),
        (
            "tip.revert",
            "Cofnij ostatnią zmianę bieżącego patcha",
            "Undo the last change to the current patch",
        ),
        (
            "tip.copy_library",
            "Skopiuj otwarty slot urządzenia do Biblioteki jako trwały wpis",
            "Copy the open device slot to the Library as a permanent entry",
        ),
        (
            "error.title",
            "Operacja nie powiodła się",
            "Operation failed",
        ),
        // --- Urządzenie / nagłówek ---
        (
            "device.qt_warning",
            "⚠ QuickTone działa — sync presetu może być niepełny",
            "⚠ QuickTone is running — preset sync may be incomplete",
        ),
        (
            "device.connect_hint",
            "Podłącz urządzenie i użyj przycisku Pobierz w nagłówku",
            "Connect the device and use the Fetch button in the header",
        ),
        (
            "device.connect_to_control",
            "Podłącz urządzenie, by sterować",
            "Connect the device to control it",
        ),
        // --- Wspólne akcje ---
        ("action.clear", "Wyczyść", "Clear"),
        ("action.save_file", "Zapisz…", "Save…"),
        ("action.open", "Otwórz", "Open"),
        ("action.add", "Dodaj", "Add"),
        ("action.create", "Utwórz", "Create"),
        ("action.set", "Ustaw", "Set"),
        // --- Monitor MIDI ---
        ("midi.listen", "Nasłuch", "Listen"),
        (
            "midi.active",
            "Nasłuch aktywny — operuj urządzeniem",
            "Listening — operate the device",
        ),
        (
            "midi.idle",
            "Włącz nasłuch i operuj urządzeniem",
            "Turn on listening and operate the device",
        ),
        (
            "midi.no_device",
            "Podłącz urządzenie, by nasłuchiwać MIDI",
            "Connect the device to listen to MIDI",
        ),
        ("midi.empty", "(brak komunikatów)", "(no messages)"),
        // --- DRUM ---
        (
            "drum.hint",
            "Sterowanie perkusją (MIDI CC → urządzenie)",
            "Drum control (MIDI CC → device)",
        ),
        ("drum.tempo", "Tempo (BPM)", "Tempo (BPM)"),
        ("drum.volume", "Głośność", "Volume"),
        ("drum.group", "Wzorzec — grupa", "Pattern — group"),
        ("drum.pattern", "Wzorzec", "Pattern"),
        // --- Metadane patcha ---
        ("inspector.meta", "Meta", "Meta"),
        (
            "meta.no_selection",
            "Wybierz patch, by opisać jego autorstwo i pochodzenie.",
            "Select a patch to describe its authorship and origin.",
        ),
        ("meta.author", "Autor", "Author"),
        ("meta.source", "Źródło (paczka)", "Source (pack)"),
        ("meta.source_url", "Adres źródła", "Source address"),
        (
            "meta.license",
            "Licencja / warunki użycia",
            "License / terms of use",
        ),
        ("meta.notes", "Notatki", "Notes"),
        ("meta.rating", "Ocena", "Rating"),
        ("meta.no_rating", "bez oceny", "no rating"),
        ("meta.favorite", "Ulubiony", "Favorite"),
        ("meta.save", "Zapisz metadane", "Save metadata"),
        ("meta.tags", "Tagi", "Tags"),
        ("meta.collections", "Kolekcje", "Collections"),
        ("meta.ph_author", "np. Jimmy Lin", "e.g. Jimmy Lin"),
        (
            "meta.ph_source",
            "np. JL-British Pack",
            "e.g. JL-British Pack",
        ),
        (
            "meta.ph_license",
            "np. darmowe, bez redystrybucji",
            "e.g. free, no redistribution",
        ),
        ("meta.ph_tag", "np. metal", "e.g. metal"),
        ("meta.ph_collection", "nowa kolekcja", "new collection"),
        // --- Katalog źródeł patchy ---
        ("inspector.sources", "Źródła", "Sources"),
        (
            "sources.intro",
            "Gdzie zdobyć patche MG-101. Aplikacja nie zawiera cudzych plików — żadne z tych źródeł nie daje licencji na redystrybucję. Pobierz od autora, potem zaimportuj (import folderu tworzy kolekcję i zapisuje źródło).",
            "Where to get MG-101 patches. This app bundles no third-party files — none of these sources grants redistribution rights. Download from the author, then import (importing a folder creates a collection and records the source).",
        ),
        ("sources.paid", "płatne", "paid"),
        ("sources.author", "Autor:", "Author:"),
        ("sources.license", "Licencja:", "License:"),
        (
            "sources.open",
            "Otwórz stronę autora",
            "Open the author's page",
        ),
        (
            "slot.hint",
            "Klik slotu = otwórz i skopiuj do Biblioteki do edycji",
            "Click a slot to open it and copy it to the Library for editing",
        ),
        ("chat.role_tool", "narzędzie", "tool"),
        ("chat.role_assistant", "asystent", "assistant"),
        ("inspector.fields", "pól", "fields"),
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

    /// Regresja: nowe sekcje UI (Meta, Źródła, DRUM, MIDI) miały tekst wpisany na
    /// sztywno po polsku — przy EN zostawał polski. Ten test pilnuje, że KAŻDY klucz
    /// używany przez UI istnieje i ma OBA tłumaczenia, i że PL≠EN tam, gdzie powinno.
    #[test]
    fn all_ui_keys_exist_in_both_languages() {
        const KEYS: &[&str] = &[
            "device.qt_warning", "device.connect_hint", "device.connect_to_control",
            "device.none", "slot.hint", "inspector.fields",
            "chat.role_tool", "chat.role_assistant",
            "action.clear", "action.save_file", "action.open", "action.add",
            "action.create", "action.set",
            "midi.listen", "midi.active", "midi.idle", "midi.no_device", "midi.empty",
            "drum.hint", "drum.tempo", "drum.volume", "drum.group", "drum.pattern",
            "inspector.meta", "meta.no_selection", "meta.author", "meta.source",
            "meta.source_url", "meta.license", "meta.notes", "meta.rating",
            "meta.no_rating", "meta.favorite", "meta.save", "meta.tags",
            "meta.collections", "meta.ph_author", "meta.ph_source", "meta.ph_license",
            "meta.ph_tag", "meta.ph_collection",
            "inspector.sources", "sources.intro", "sources.paid", "sources.author",
            "sources.license", "sources.open",
        ];
        for key in KEYS {
            let pl = tr(Lang::Pl, key);
            let en = tr(Lang::En, key);
            assert_ne!(pl, "??", "brak klucza {key} (UI pokaże ??)");
            assert!(!pl.is_empty() && !en.is_empty(), "{key}: puste tłumaczenie");
            // Te akurat MUSZĄ się różnić — gdyby były identyczne, znaczyłoby to, że
            // ktoś wkleił polski tekst także do kolumny EN.
            if !matches!(*key, "inspector.meta" | "drum.tempo") {
                assert_ne!(pl, en, "{key}: PL i EN identyczne — czy EN na pewno przetłumaczone?");
            }
        }
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

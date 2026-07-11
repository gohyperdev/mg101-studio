//! Przechowalnia klucza API w systemowym magazynie sekretów (Keychain na macOS,
//! Credential Manager na Windows) — parytet v1 `KeychainStore`.
//!
//! Ta sama pozycja co v1: **service `dev.mos.mg101studio.ai`**, **account =
//! nazwa dostawcy** (`anthropic`/`openai`). Dzięki temu klucz zapisany w Swift
//! v1 jest odczytywany przez wersję Rust bez ponownego wpisywania.
//!
//! Natywny (nie wchodzi do wasm).

use mg101_agent_core::Provider;

/// Usługa (kSecAttrService) — identyczna z v1.
const SERVICE: &str = "dev.mos.mg101studio.ai";

/// Konto (kSecAttrAccount) dla dostawcy — identyczne z v1.
fn account(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "anthropic",
        Provider::OpenAiCompatible => "openai",
    }
}

fn entry(provider: Provider) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, account(provider)).ok()
}

/// Odczytuje klucz API dostawcy z magazynu sekretów. `None`, gdy brak wpisu lub
/// magazyn niedostępny (nie jest to błąd — użytkownik wpisze klucz ręcznie).
///
/// UWAGA: na macOS wywołanie może **zablokować** wątek do czasu decyzji w oknie
/// dostępu do Keychain (przy nowym/przebudowanym binarium). Nie wołaj na wątku UI
/// przed pokazaniem okna — użyj [`load_key_async`].
pub fn load_key(provider: Provider) -> Option<String> {
    let key = entry(provider)?.get_password().ok()?;
    let key = key.trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// Wersja nieblokująca: odczyt klucza na wątku w tle. Zwraca odbiornik, z którego
/// UI pobierze wynik, gdy magazyn odpowie (okno pojawia się natychmiast, nawet
/// gdy macOS czeka na zgodę dostępu do Keychain).
pub fn load_key_async(provider: Provider) -> std::sync::mpsc::Receiver<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load_key(provider));
    });
    rx
}

/// Zapisuje (lub usuwa, gdy pusty) klucz API dostawcy — persystencja jak v1.
/// Zwraca komunikat błędu przy niepowodzeniu (magazyn nie zawsze dostępny).
pub fn save_key(provider: Provider, key: &str) -> Result<(), String> {
    let Some(entry) = entry(provider) else {
        return Err("magazyn sekretów niedostępny".into());
    };
    let key = key.trim();
    let res = if key.is_empty() {
        // Usunięcie pustego (parytet: pusty klucz kasuje wpis).
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        entry.set_password(key)
    };
    res.map_err(|e| e.to_string())
}

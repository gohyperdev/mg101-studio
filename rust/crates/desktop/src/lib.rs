//! Powłoka desktop MG101 Studio (macOS + Windows).
//!
//! Warstwa logiki UI ([`vm`]) i i18n ([`i18n`]) jest bez zależności od Slint —
//! testowalna cross-platform i wpięta w Slint dopiero w binarce (`main`). Cały
//! stan przechodzi przez magistralę [`mg101_studio::Studio::execute`] (te same
//! komendy co agent/MCP — ADR-0002).

pub mod i18n;
pub mod vm;

// Wątek agenta wymaga HTTP + wątków — tylko natywnie (nie wasm).
#[cfg(not(target_arch = "wasm32"))]
pub mod agent;

// Klucz API w systemowym magazynie sekretów — tylko natywnie.
#[cfg(not(target_arch = "wasm32"))]
pub mod keychain;

// Zrzut banków z fizycznego urządzenia + rejestr packów — tylko natywnie.
#[cfg(not(target_arch = "wasm32"))]
pub mod device;

pub use i18n::{tr, Lang};
pub use vm::{
    ByteChange, ChatRow, LibraryTab, ModelOption, PatchDetail, PatchRow, SlotRow, ViewModel,
};

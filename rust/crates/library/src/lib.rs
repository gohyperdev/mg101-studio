//! Biblioteka patchy — model, fingerprint, provenance, stan trójdrożny (HLD §3-4).
//!
//! Zasada (ADR-0002): biblioteka jest **device-agnostyczna**. Zna patch jako
//! nieprzezroczysty **blob** (bajty święte — oryginał zachowany bez straty),
//! zestaw hashy (fingerprint) i metadane (tagi, grupy, pochodzenie). Nie zna
//! struktury kanonicznej ani MG-101 — dekodowanie należy do Device Packa.
//!
//! Podział jak w WAL: **logika** (typy, fingerprint, silnik stanu sync) czysta i
//! testowalna; **skład** za traitem [`LibraryStore`] (in-memory teraz, SQLite w
//! kolejnym kroku).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod fingerprint;
#[cfg(not(target_arch = "wasm32"))]
mod sqlite;
mod store;
mod sync;

pub use fingerprint::{content_hash, exact_hash, MaskRange};
#[cfg(not(target_arch = "wasm32"))]
pub use sqlite::SqliteStore;
pub use store::{LibraryError, LibraryStore, MemoryStore};
pub use sync::{slot_sync_state, SlotView, SyncState};

/// Identyfikator patcha biblioteki (UUID w formie tekstowej).
pub type PatchId = String;
/// Identyfikator tagu (swobodna etykieta, np. `"metal"`).
pub type TagId = String;
/// Identyfikator grupy (uporządkowanej kolekcji).
pub type GroupId = String;
/// Identyfikator rodziny/urządzenia docelowego (np. `"nux-mg101"`).
pub type DeviceId = String;
/// Hash treści w hex (SHA256).
pub type Hash = String;

pub use mg101_device_pack_api::SlotAddr;

/// Pochodzenie patcha (ślad źródła — HLD §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchOrigin {
    /// Zaimportowany z pliku `.mg101patch`.
    ImportedFile,
    /// Ściągnięty ze slotu urządzenia.
    PulledFromDevice,
    /// Utworzony w aplikacji.
    Created,
    /// Otrzymany/współdzielony (chmura, wymiana).
    Shared,
}

/// Wpis biblioteki (HLD §3). `blob` to oryginalne bajty (bajty święte);
/// `content_hash` (fingerprint brzmieniowy, znormalizowany) i `exact_hash`
/// (bit-identyczność całego blobu) służą dopasowaniu po połączeniu (§4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryPatch {
    pub id: PatchId,
    pub name: String,
    /// Oryginalne bajty patcha — nieznane bajty zachowane bez straty.
    pub blob: Vec<u8>,
    /// Bajty bazowe (stan z chwili utworzenia/importu) — baza dla widoku „Zmiany".
    /// `serde(default)` = kompatybilność wstecz (stare wpisy bez pola → pusty →
    /// diff liczony względem bieżącego blobu, czyli brak fałszywych zmian).
    #[serde(default)]
    pub baseline_blob: Vec<u8>,
    pub origin: PatchOrigin,
    pub device_id: DeviceId,
    pub firmware: Option<String>,
    pub codec_version: String,
    /// Fingerprint brzmieniowy (znormalizowany — bez nazwy i pól nieistotnych).
    pub content_hash: Hash,
    /// Hash całego blobu (bit-identyczność).
    pub exact_hash: Hash,
    pub tags: BTreeSet<TagId>,
    pub groups: BTreeSet<GroupId>,
    /// Znacznik utworzenia (ms epoch — dostarczany przez wywołującego, wasm-safe).
    pub created_at: i64,
    pub updated_at: i64,
    /// Rewizja (ADR-0001) — bazowa dla sync lokalne↔chmura.
    pub revision: u64,
}

/// Uporządkowana kolekcja patchy (HLD §3). Kolejność w `members` wyznacza
/// kolejność zapisu przy transferze grupowym (E5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// Uporządkowana lista patchy. Patch może należeć do wielu grup.
    pub members: Vec<PatchId>,
}

impl Group {
    /// Dodaje patch na koniec (bez duplikatów — utrzymuje unikalność).
    pub fn push_unique(&mut self, patch: PatchId) {
        if !self.members.contains(&patch) {
            self.members.push(patch);
        }
    }

    /// Usuwa patch z grupy (zachowuje kolejność pozostałych).
    pub fn remove(&mut self, patch: &PatchId) {
        self.members.retain(|p| p != patch);
    }
}

/// Ślad transferu Biblioteka→slot (provenance, HLD §4). Pozwala policzyć stan
/// trójdrożny po ponownym połączeniu konkretnego egzemplarza urządzenia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSlotLink {
    /// Stabilny identyfikator egzemplarza (np. USB serial).
    pub device_serial: DeviceId,
    pub slot: SlotAddr,
    pub library_patch_id: PatchId,
    /// Hash (exact) patcha w chwili transferu — baza do wykrycia zmian.
    pub hash_at_transfer: Hash,
    pub transferred_at: i64,
}

/// Indeks biblioteki do dopasowania po hashu (budowany ze store dla silnika sync).
#[derive(Debug, Default)]
pub struct LibraryIndex {
    by_id: BTreeMap<PatchId, (Hash, Hash, u64)>, // id → (content, exact, revision)
    by_exact: BTreeMap<Hash, PatchId>,
    by_content: BTreeMap<Hash, PatchId>,
}

impl LibraryIndex {
    /// Buduje indeks z listy patchy.
    pub fn build<'a>(patches: impl IntoIterator<Item = &'a LibraryPatch>) -> Self {
        let mut idx = LibraryIndex::default();
        for p in patches {
            idx.by_id.insert(
                p.id.clone(),
                (p.content_hash.clone(), p.exact_hash.clone(), p.revision),
            );
            idx.by_exact.insert(p.exact_hash.clone(), p.id.clone());
            idx.by_content
                .entry(p.content_hash.clone())
                .or_insert_with(|| p.id.clone());
        }
        idx
    }

    /// (content_hash, exact_hash) patcha o danym id.
    pub fn hashes(&self, id: &PatchId) -> Option<(&Hash, &Hash)> {
        self.by_id.get(id).map(|(c, e, _)| (c, e))
    }

    /// Id patcha o dokładnie tym blobie (bit-identyczny).
    pub fn find_exact(&self, exact: &Hash) -> Option<&PatchId> {
        self.by_exact.get(exact)
    }

    /// Id patcha o tym samym fingerprincie brzmieniowym.
    pub fn find_content(&self, content: &Hash) -> Option<&PatchId> {
        self.by_content.get(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_push_unique_and_remove_preserve_order() {
        let mut g = Group {
            id: "g1".into(),
            name: "Koncert".into(),
            members: vec![],
        };
        g.push_unique("a".into());
        g.push_unique("b".into());
        g.push_unique("a".into()); // duplikat ignorowany
        assert_eq!(g.members, vec!["a", "b"]);
        g.remove(&"a".into());
        assert_eq!(g.members, vec!["b"]);
    }

    #[test]
    fn index_matches_by_exact_and_content() {
        let p = LibraryPatch {
            id: "p1".into(),
            name: "X".into(),
            baseline_blob: Vec::new(),
            blob: vec![1, 2, 3],
            origin: PatchOrigin::Created,
            device_id: "nux-mg101".into(),
            firmware: None,
            codec_version: "1".into(),
            content_hash: "c1".into(),
            exact_hash: "e1".into(),
            tags: BTreeSet::new(),
            groups: BTreeSet::new(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
        };
        let idx = LibraryIndex::build([&p]);
        assert_eq!(idx.find_exact(&"e1".into()), Some(&"p1".to_string()));
        assert_eq!(idx.find_content(&"c1".into()), Some(&"p1".to_string()));
        assert_eq!(idx.hashes(&"p1".into()), Some((&"c1".into(), &"e1".into())));
        assert!(idx.find_exact(&"nope".into()).is_none());
    }
}

//! Silnik stanu trójdrożnego slot ↔ Biblioteka (HLD §4).
//!
//! Czysta funkcja: po połączeniu urządzenia dla każdego slotu porównuje bieżący
//! hash slotu z linkiem provenance ([`DeviceSlotLink`]) i z biblioteką
//! ([`LibraryIndex`]) → [`SyncState`]. Bez we/wy, w pełni testowalna.

use crate::{DeviceSlotLink, Hash, LibraryIndex, PatchId, SlotAddr};

/// Bieżący stan slotu urządzenia widziany po odczycie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotView {
    pub addr: SlotAddr,
    /// `(content_hash, exact_hash)` zawartości slotu, lub `None` gdy slot pusty.
    pub content: Option<(Hash, Hash)>,
    /// Czy slot jest zapisywalny (Factory = false).
    pub writable: bool,
}

/// Stan synchronizacji slotu względem Biblioteki (HLD §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    /// Slot pusty/domyślny — cel transferu.
    Empty,
    /// Slot = powiązany patch z Biblioteki (hash zgodny).
    InSync { patch_id: PatchId },
    /// Slot był z Biblioteki, ale zmieniony na urządzeniu.
    DeviceModified { patch_id: PatchId },
    /// Powiązany wpis w Bibliotece nowszy niż slot.
    LibraryNewer { patch_id: PatchId },
    /// Slot nie pasuje do niczego w Bibliotece.
    DeviceOnly,
}

/// Liczy stan trójdrożny dla jednego slotu.
///
/// `link` to zapisany ślad transferu **dla tego slotu na tym egzemplarzu** (albo
/// `None`, gdy nigdy nie transferowano lub link wskazuje usunięty patch — wtedy
/// spadamy do dopasowania po hashu). Kolejność decyzji odwzorowuje tabelę HLD §4.
pub fn slot_sync_state(
    slot: &SlotView,
    link: Option<&DeviceSlotLink>,
    index: &LibraryIndex,
) -> SyncState {
    // Slot pusty → cel transferu.
    let Some((slot_content, slot_exact)) = &slot.content else {
        return SyncState::Empty;
    };

    // Link z żywym patchem w Bibliotece → porównanie względem chwili transferu.
    if let Some(link) = link {
        if let Some((_lib_content, lib_exact)) = index.hashes(&link.library_patch_id) {
            if slot_exact == lib_exact {
                return SyncState::InSync {
                    patch_id: link.library_patch_id.clone(),
                };
            }
            let slot_changed = *slot_exact != link.hash_at_transfer;
            let lib_changed = *lib_exact != link.hash_at_transfer;
            if slot_changed {
                // Urządzenie odbiega od stanu z transferu (nadrzędne nad lib).
                return SyncState::DeviceModified {
                    patch_id: link.library_patch_id.clone(),
                };
            }
            if lib_changed {
                // Slot jak w chwili transferu, ale Biblioteka edytowana później.
                return SyncState::LibraryNewer {
                    patch_id: link.library_patch_id.clone(),
                };
            }
            // Oba == hash_at_transfer ⇒ równe (obsłużone wyżej przez slot==lib).
        }
        // Link wisi (patch usunięty z Biblioteki) → dopasowanie po hashu niżej.
    }

    // Brak (użytecznego) linku: dopasuj po treści. Bit-identyczny ma pierwszeństwo,
    // potem dopasowanie brzmieniowe (znormalizowane). Inaczej — tylko na urządzeniu.
    if let Some(id) = index.find_exact(slot_exact) {
        return SyncState::InSync {
            patch_id: id.clone(),
        };
    }
    if let Some(id) = index.find_content(slot_content) {
        return SyncState::InSync {
            patch_id: id.clone(),
        };
    }
    SyncState::DeviceOnly
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryPatch, PatchOrigin};
    use std::collections::BTreeSet;

    fn patch(id: &str, content: &str, exact: &str, revision: u64) -> LibraryPatch {
        LibraryPatch {
            id: id.into(),
            name: id.into(),
            blob: vec![],
            origin: PatchOrigin::Created,
            device_id: "nux-mg101".into(),
            firmware: None,
            codec_version: "1".into(),
            content_hash: content.into(),
            exact_hash: exact.into(),
            tags: BTreeSet::new(),
            groups: BTreeSet::new(),
            created_at: 0,
            updated_at: 0,
            revision,
        }
    }

    fn slot(content: Option<(&str, &str)>, writable: bool) -> SlotView {
        SlotView {
            addr: SlotAddr {
                bank: "user".into(),
                index: 0,
            },
            content: content.map(|(c, e)| (c.into(), e.into())),
            writable,
        }
    }

    fn link(patch_id: &str, hash_at_transfer: &str) -> DeviceSlotLink {
        DeviceSlotLink {
            device_serial: "SN1".into(),
            slot: SlotAddr {
                bank: "user".into(),
                index: 0,
            },
            library_patch_id: patch_id.into(),
            hash_at_transfer: hash_at_transfer.into(),
            transferred_at: 0,
        }
    }

    #[test]
    fn empty_slot_is_empty() {
        let idx = LibraryIndex::build([]);
        assert_eq!(
            slot_sync_state(&slot(None, true), None, &idx),
            SyncState::Empty
        );
    }

    #[test]
    fn in_sync_when_slot_matches_linked_patch() {
        let p = patch("p1", "c1", "e1", 1);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("c1", "e1")), true);
        let l = link("p1", "e1");
        assert_eq!(
            slot_sync_state(&s, Some(&l), &idx),
            SyncState::InSync {
                patch_id: "p1".into()
            }
        );
    }

    #[test]
    fn device_modified_when_slot_diverged_since_transfer() {
        // Patch w bibliotece niezmieniony (exact == hash_at_transfer), slot inny.
        let p = patch("p1", "c1", "e1", 1);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("c9", "e9")), true); // slot zmieniony na urządzeniu
        let l = link("p1", "e1");
        assert_eq!(
            slot_sync_state(&s, Some(&l), &idx),
            SyncState::DeviceModified {
                patch_id: "p1".into()
            }
        );
    }

    #[test]
    fn library_newer_when_slot_unchanged_but_library_edited() {
        // hash_at_transfer = e0; biblioteka teraz e2 (edytowana), slot nadal e0.
        let p = patch("p1", "c2", "e2", 5);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("c0", "e0")), true);
        let l = link("p1", "e0");
        assert_eq!(
            slot_sync_state(&s, Some(&l), &idx),
            SyncState::LibraryNewer {
                patch_id: "p1".into()
            }
        );
    }

    #[test]
    fn both_changed_resolves_to_device_modified() {
        // Slot i Biblioteka odjechały od chwili transferu w RÓŻNE strony.
        // Wg tabeli HLD §4 (5 stanów) urządzenie jest nadrzędne → DeviceModified.
        // (Świadome: brak stanu Conflict — decyzja per konflikt należy do E5.)
        let p = patch("p1", "cLIB", "eLIB", 7);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("cDEV", "eDEV")), true);
        let l = link("p1", "eTRANSFER");
        assert_eq!(
            slot_sync_state(&s, Some(&l), &idx),
            SyncState::DeviceModified {
                patch_id: "p1".into()
            }
        );
    }

    #[test]
    fn dangling_link_falls_back_to_content_match() {
        // Link wskazuje usunięty patch; slot pasuje treścią do innego wpisu.
        let other = patch("p2", "cX", "eX", 1);
        let idx = LibraryIndex::build([&other]);
        let s = slot(Some(("cX", "eX")), true);
        let l = link("deleted", "eOld");
        assert_eq!(
            slot_sync_state(&s, Some(&l), &idx),
            SyncState::InSync {
                patch_id: "p2".into()
            }
        );
    }

    #[test]
    fn device_only_when_nothing_matches() {
        let p = patch("p1", "c1", "e1", 1);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("cZ", "eZ")), false); // np. Factory nieznany bibliotece
        assert_eq!(slot_sync_state(&s, None, &idx), SyncState::DeviceOnly);
    }

    #[test]
    fn content_match_without_link_is_in_sync() {
        // Bit-różny, ale brzmieniowo ten sam (ta sama nazwa-niezależna treść).
        let p = patch("p1", "cSAME", "eLIB", 1);
        let idx = LibraryIndex::build([&p]);
        let s = slot(Some(("cSAME", "eDEV")), true);
        assert_eq!(
            slot_sync_state(&s, None, &idx),
            SyncState::InSync {
                patch_id: "p1".into()
            }
        );
    }
}

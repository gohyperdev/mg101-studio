//! Skład Biblioteki — trait [`LibraryStore`] + implementacja in-memory.
//!
//! Trait odgradza logikę (silnik sync, transfer w E5) od backendu składu
//! (in-memory teraz, SQLite/Postgres później — HLD §3). Mutacje patchy używają
//! **rewizji optymistycznych** (ADR-0001): zapis wymaga oczekiwanej rewizji,
//! inaczej [`LibraryError::RevisionConflict`].

use crate::{DeviceSlotLink, Group, GroupId, LibraryIndex, LibraryPatch, PatchId, SlotAddr, TagId};
use std::collections::BTreeMap;

/// Błąd operacji na Bibliotece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    /// Patch/grupa o danym id nie istnieje.
    NotFound(String),
    /// Patch o tym id już istnieje (przy `add`).
    Duplicate(PatchId),
    /// Zapis odrzucony — oczekiwana rewizja ≠ bieżąca (współbieżna edycja).
    RevisionConflict {
        patch_id: PatchId,
        expected: u64,
        actual: u64,
    },
    /// Awaria backendu składu (we/wy, (de)serializacja) — nie mylić z NotFound.
    Backend(String),
    /// Niepoprawne argumenty operacji (np. reorder nie jest permutacją).
    InvalidInput(String),
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::NotFound(id) => write!(f, "nie znaleziono: {id}"),
            LibraryError::Duplicate(id) => write!(f, "patch już istnieje: {id}"),
            LibraryError::RevisionConflict {
                patch_id,
                expected,
                actual,
            } => write!(
                f,
                "konflikt rewizji patcha {patch_id}: oczekiwano {expected}, jest {actual}"
            ),
            LibraryError::Backend(m) => write!(f, "awaria składu: {m}"),
            LibraryError::InvalidInput(m) => write!(f, "niepoprawne dane: {m}"),
        }
    }
}

impl std::error::Error for LibraryError {}

/// Czy `order` jest permutacją `members` (ten sam multizbiór elementów).
pub(crate) fn is_permutation(order: &[PatchId], members: &[PatchId]) -> bool {
    if order.len() != members.len() {
        return false;
    }
    let mut a = order.to_vec();
    let mut b = members.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Kontrakt składu Biblioteki. Silniki (sync, transfer) i UI zależą od tego
/// traitu, nie od konkretnego backendu.
pub trait LibraryStore {
    // --- Patche ---
    fn add(&mut self, patch: LibraryPatch) -> Result<(), LibraryError>;
    fn get(&self, id: &PatchId) -> Option<LibraryPatch>;
    /// Nadpisuje patch z kontrolą rewizji; zwraca nową rewizję (bieżąca + 1).
    fn update(&mut self, patch: LibraryPatch, expected_revision: u64) -> Result<u64, LibraryError>;
    fn remove(&mut self, id: &PatchId) -> Result<LibraryPatch, LibraryError>;
    fn all(&self) -> Vec<LibraryPatch>;

    // --- Tagi ---
    fn add_tag(&mut self, id: &PatchId, tag: TagId) -> Result<(), LibraryError>;
    fn remove_tag(&mut self, id: &PatchId, tag: &TagId) -> Result<(), LibraryError>;
    fn by_tag(&self, tag: &TagId) -> Vec<LibraryPatch>;

    // --- Grupy ---
    fn create_group(&mut self, group: Group) -> Result<(), LibraryError>;
    fn group(&self, id: &GroupId) -> Option<Group>;
    fn groups(&self) -> Vec<Group>;
    fn delete_group(&mut self, id: &GroupId) -> Result<(), LibraryError>;
    /// Dodaje patch na koniec grupy (uporządkowanej), aktualizuje przynależność.
    fn add_to_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError>;
    fn remove_from_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError>;
    /// Ustawia nową kolejność członków grupy (musi być permutacją obecnych).
    fn reorder_group(&mut self, group: &GroupId, order: Vec<PatchId>) -> Result<(), LibraryError>;

    // --- Provenance ---
    /// Zapisuje ślad transferu. Zwraca błąd, gdy backend nie utrwali linku
    /// (cichy zanik provenance fałszowałby stany sync przy następnym połączeniu).
    fn record_link(&mut self, link: DeviceSlotLink) -> Result<(), LibraryError>;
    fn links_for_device(&self, serial: &str) -> Vec<DeviceSlotLink>;
    fn link_for_slot(&self, serial: &str, slot: &SlotAddr) -> Option<DeviceSlotLink>;

    /// Indeks do dopasowania po hashu (silnik sync). Domyślnie z `all()`.
    fn index(&self) -> LibraryIndex {
        let patches = self.all();
        LibraryIndex::build(patches.iter())
    }
}

/// Prosta implementacja w pamięci — do testów, prototypu UI i jako referencja
/// kontraktu dla backendu trwałego (SQLite).
#[derive(Debug, Default)]
pub struct MemoryStore {
    patches: BTreeMap<PatchId, LibraryPatch>,
    groups: BTreeMap<GroupId, Group>,
    links: Vec<DeviceSlotLink>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl LibraryStore for MemoryStore {
    fn add(&mut self, patch: LibraryPatch) -> Result<(), LibraryError> {
        if self.patches.contains_key(&patch.id) {
            return Err(LibraryError::Duplicate(patch.id));
        }
        self.patches.insert(patch.id.clone(), patch);
        Ok(())
    }

    fn get(&self, id: &PatchId) -> Option<LibraryPatch> {
        self.patches.get(id).cloned()
    }

    fn update(
        &mut self,
        mut patch: LibraryPatch,
        expected_revision: u64,
    ) -> Result<u64, LibraryError> {
        let current = self
            .patches
            .get(&patch.id)
            .ok_or_else(|| LibraryError::NotFound(patch.id.clone()))?;
        if current.revision != expected_revision {
            return Err(LibraryError::RevisionConflict {
                patch_id: patch.id,
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let new_revision = current.revision + 1;
        patch.revision = new_revision;
        self.patches.insert(patch.id.clone(), patch);
        Ok(new_revision)
    }

    fn remove(&mut self, id: &PatchId) -> Result<LibraryPatch, LibraryError> {
        let removed = self
            .patches
            .remove(id)
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        // Sprzątanie przynależności do grup (spójność).
        for g in self.groups.values_mut() {
            g.remove(id);
        }
        Ok(removed)
    }

    fn all(&self) -> Vec<LibraryPatch> {
        self.patches.values().cloned().collect()
    }

    fn add_tag(&mut self, id: &PatchId, tag: TagId) -> Result<(), LibraryError> {
        let p = self
            .patches
            .get_mut(id)
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        p.tags.insert(tag);
        Ok(())
    }

    fn remove_tag(&mut self, id: &PatchId, tag: &TagId) -> Result<(), LibraryError> {
        let p = self
            .patches
            .get_mut(id)
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        p.tags.remove(tag);
        Ok(())
    }

    fn by_tag(&self, tag: &TagId) -> Vec<LibraryPatch> {
        self.patches
            .values()
            .filter(|p| p.tags.contains(tag))
            .cloned()
            .collect()
    }

    fn create_group(&mut self, group: Group) -> Result<(), LibraryError> {
        if self.groups.contains_key(&group.id) {
            return Err(LibraryError::Duplicate(group.id));
        }
        self.groups.insert(group.id.clone(), group);
        Ok(())
    }

    fn group(&self, id: &GroupId) -> Option<Group> {
        self.groups.get(id).cloned()
    }

    fn groups(&self) -> Vec<Group> {
        self.groups.values().cloned().collect()
    }

    fn delete_group(&mut self, id: &GroupId) -> Result<(), LibraryError> {
        self.groups
            .remove(id)
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        // Usuń przynależność z patchy.
        for p in self.patches.values_mut() {
            p.groups.remove(id);
        }
        Ok(())
    }

    fn add_to_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError> {
        if !self.patches.contains_key(patch) {
            return Err(LibraryError::NotFound(patch.clone()));
        }
        let g = self
            .groups
            .get_mut(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        g.push_unique(patch.clone());
        // Zwierciadło przynależności na patchu.
        self.patches
            .get_mut(patch)
            .expect("patch istnieje (sprawdzone wyżej)")
            .groups
            .insert(group.clone());
        Ok(())
    }

    fn remove_from_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError> {
        let g = self
            .groups
            .get_mut(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        g.remove(patch);
        if let Some(p) = self.patches.get_mut(patch) {
            p.groups.remove(group);
        }
        Ok(())
    }

    fn reorder_group(&mut self, group: &GroupId, order: Vec<PatchId>) -> Result<(), LibraryError> {
        let g = self
            .groups
            .get_mut(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        // Nowa kolejność musi być PERMUTACJĄ obecnych członków (multizbiór, nie
        // tylko podzbiór — inaczej ["a","a"] przeszłoby dla {a,b} i zgubiło b).
        if !is_permutation(&order, &g.members) {
            return Err(LibraryError::InvalidInput(format!(
                "kolejność nie jest permutacją członków grupy {group}"
            )));
        }
        g.members = order;
        Ok(())
    }

    fn record_link(&mut self, link: DeviceSlotLink) -> Result<(), LibraryError> {
        // Jeden aktualny link per (egzemplarz, slot) — zastąp poprzedni.
        self.links
            .retain(|l| !(l.device_serial == link.device_serial && l.slot == link.slot));
        self.links.push(link);
        Ok(())
    }

    fn links_for_device(&self, serial: &str) -> Vec<DeviceSlotLink> {
        self.links
            .iter()
            .filter(|l| l.device_serial == serial)
            .cloned()
            .collect()
    }

    fn link_for_slot(&self, serial: &str, slot: &SlotAddr) -> Option<DeviceSlotLink> {
        self.links
            .iter()
            .find(|l| l.device_serial == serial && &l.slot == slot)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PatchOrigin;
    use std::collections::BTreeSet;

    fn patch(id: &str) -> LibraryPatch {
        LibraryPatch {
            id: id.into(),
            name: id.into(),
            baseline_blob: Vec::new(),
            blob: vec![1, 2, 3],
            origin: PatchOrigin::Created,
            device_id: "nux-mg101".into(),
            firmware: None,
            codec_version: "1".into(),
            content_hash: format!("c-{id}"),
            exact_hash: format!("e-{id}"),
            tags: BTreeSet::new(),
            groups: BTreeSet::new(),
            meta: Default::default(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
        }
    }

    #[test]
    fn add_get_duplicate() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        assert_eq!(s.get(&"p1".into()).unwrap().name, "p1");
        assert_eq!(
            s.add(patch("p1")),
            Err(LibraryError::Duplicate("p1".into()))
        );
    }

    #[test]
    fn update_enforces_revision() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        let mut edited = patch("p1");
        edited.name = "nowa".into();
        // Zła oczekiwana rewizja → konflikt.
        assert!(matches!(
            s.update(edited.clone(), 99),
            Err(LibraryError::RevisionConflict { actual: 1, .. })
        ));
        // Poprawna → nowa rewizja 2.
        let rev = s.update(edited, 1).unwrap();
        assert_eq!(rev, 2);
        assert_eq!(s.get(&"p1".into()).unwrap().revision, 2);
    }

    #[test]
    fn tags_add_remove_filter() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        s.add(patch("p2")).unwrap();
        s.add_tag(&"p1".into(), "metal".into()).unwrap();
        s.add_tag(&"p2".into(), "metal".into()).unwrap();
        s.add_tag(&"p1".into(), "live".into()).unwrap();
        assert_eq!(s.by_tag(&"metal".into()).len(), 2);
        assert_eq!(s.by_tag(&"live".into()).len(), 1);
        s.remove_tag(&"p1".into(), &"metal".into()).unwrap();
        assert_eq!(s.by_tag(&"metal".into()).len(), 1);
    }

    #[test]
    fn groups_ordered_membership_mirrored() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        s.add(patch("p2")).unwrap();
        s.create_group(Group {
            id: "g1".into(),
            name: "Koncert".into(),
            members: vec![],
        })
        .unwrap();
        s.add_to_group(&"g1".into(), &"p1".into()).unwrap();
        s.add_to_group(&"g1".into(), &"p2".into()).unwrap();
        assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p1", "p2"]);
        // Zwierciadło na patchu.
        assert!(s.get(&"p1".into()).unwrap().groups.contains("g1"));
        // Reorder.
        s.reorder_group(&"g1".into(), vec!["p2".into(), "p1".into()])
            .unwrap();
        assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p2", "p1"]);
        // Reorder niepermutacją → błąd.
        assert!(s.reorder_group(&"g1".into(), vec!["p1".into()]).is_err());
    }

    #[test]
    fn removing_patch_cleans_group_membership() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        s.create_group(Group {
            id: "g1".into(),
            name: "G".into(),
            members: vec![],
        })
        .unwrap();
        s.add_to_group(&"g1".into(), &"p1".into()).unwrap();
        s.remove(&"p1".into()).unwrap();
        assert!(s.group(&"g1".into()).unwrap().members.is_empty());
    }

    #[test]
    fn deleting_group_clears_patch_membership() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        s.create_group(Group {
            id: "g1".into(),
            name: "G".into(),
            members: vec![],
        })
        .unwrap();
        s.add_to_group(&"g1".into(), &"p1".into()).unwrap();
        s.delete_group(&"g1".into()).unwrap();
        assert!(s.get(&"p1".into()).unwrap().groups.is_empty());
    }

    #[test]
    fn links_one_per_slot_and_lookup() {
        let mut s = MemoryStore::new();
        let slot = SlotAddr {
            bank: "user".into(),
            index: 3,
        };
        s.record_link(DeviceSlotLink {
            device_serial: "SN1".into(),
            slot: slot.clone(),
            library_patch_id: "p1".into(),
            hash_at_transfer: "e1".into(),
            transferred_at: 10,
        })
        .unwrap();
        // Nowy transfer do tego samego slotu zastępuje link.
        s.record_link(DeviceSlotLink {
            device_serial: "SN1".into(),
            slot: slot.clone(),
            library_patch_id: "p2".into(),
            hash_at_transfer: "e2".into(),
            transferred_at: 20,
        })
        .unwrap();
        assert_eq!(s.links_for_device("SN1").len(), 1);
        assert_eq!(
            s.link_for_slot("SN1", &slot).unwrap().library_patch_id,
            "p2"
        );
        assert!(s.link_for_slot("SN2", &slot).is_none());
    }

    #[test]
    fn index_from_store_matches_patches() {
        let mut s = MemoryStore::new();
        s.add(patch("p1")).unwrap();
        let idx = s.index();
        assert_eq!(idx.find_exact(&"e-p1".into()), Some(&"p1".to_string()));
    }
}

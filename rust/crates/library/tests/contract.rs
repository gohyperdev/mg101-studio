//! Wspólny zestaw testów kontraktu [`LibraryStore`] uruchamiany na KAŻDEJ
//! implementacji. Gwarantuje identyczną semantykę MemoryStore i SqliteStore
//! (ADR-0002: silniki mają działać na dowolnym backendzie bez zmian).

use std::collections::BTreeSet;

use mg101_library::{
    DeviceSlotLink, Group, LibraryError, LibraryPatch, LibraryStore, MemoryStore, PatchOrigin,
    SlotAddr, SqliteStore,
};

fn patch(id: &str) -> LibraryPatch {
    LibraryPatch {
        id: id.into(),
        name: id.into(),
        baseline_blob: Vec::new(),
        blob: vec![1, 2, 3, id.len() as u8],
        origin: PatchOrigin::Created,
        device_id: "nux-mg101".into(),
        firmware: None,
        codec_version: "1".into(),
        content_hash: format!("c-{id}"),
        exact_hash: format!("e-{id}"),
        tags: BTreeSet::new(),
        groups: BTreeSet::new(),
        created_at: 0,
        updated_at: 0,
        revision: 1,
    }
}

/// Pełny kontrakt składu — musi przejść identycznie dla każdej implementacji.
fn contract_suite<S: LibraryStore>(s: &mut S) {
    // add / get / duplicate
    s.add(patch("p1")).unwrap();
    s.add(patch("p2")).unwrap();
    assert_eq!(s.get(&"p1".into()).unwrap().name, "p1");
    assert_eq!(
        s.add(patch("p1")),
        Err(LibraryError::Duplicate("p1".into()))
    );
    assert_eq!(s.all().len(), 2);

    // rewizja optymistyczna
    let mut e = patch("p1");
    e.name = "nowa".into();
    assert!(matches!(
        s.update(e.clone(), 99),
        Err(LibraryError::RevisionConflict { actual: 1, .. })
    ));
    assert_eq!(s.update(e, 1).unwrap(), 2);
    assert_eq!(s.get(&"p1".into()).unwrap().revision, 2);

    // tagi
    s.add_tag(&"p1".into(), "metal".into()).unwrap();
    s.add_tag(&"p2".into(), "metal".into()).unwrap();
    assert_eq!(s.by_tag(&"metal".into()).len(), 2);
    s.remove_tag(&"p1".into(), &"metal".into()).unwrap();
    assert_eq!(s.by_tag(&"metal".into()).len(), 1);

    // grupy: kolejność + zwierciadło przynależności
    s.create_group(Group {
        id: "g1".into(),
        name: "Koncert".into(),
        members: vec![],
    })
    .unwrap();
    s.add_to_group(&"g1".into(), &"p1".into()).unwrap();
    s.add_to_group(&"g1".into(), &"p2".into()).unwrap();
    assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p1", "p2"]);
    assert!(s.get(&"p1".into()).unwrap().groups.contains("g1"));

    // reorder: permutacja OK, nie-permutacja (duplikat!) odrzucona
    s.reorder_group(&"g1".into(), vec!["p2".into(), "p1".into()])
        .unwrap();
    assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p2", "p1"]);
    assert!(matches!(
        s.reorder_group(&"g1".into(), vec!["p1".into(), "p1".into()]),
        Err(LibraryError::InvalidInput(_))
    ));

    // remove_from_group czyści zwierciadło
    s.remove_from_group(&"g1".into(), &"p1".into()).unwrap();
    assert!(!s.get(&"p1".into()).unwrap().groups.contains("g1"));
    assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p2"]);

    // provenance: jeden link per slot, lookup, brak dla innego egzemplarza
    let slot = SlotAddr {
        bank: "user".into(),
        index: 7,
    };
    s.record_link(DeviceSlotLink {
        device_serial: "SN1".into(),
        slot: slot.clone(),
        library_patch_id: "p1".into(),
        hash_at_transfer: "e-p1".into(),
        transferred_at: 1,
    })
    .unwrap();
    s.record_link(DeviceSlotLink {
        device_serial: "SN1".into(),
        slot: slot.clone(),
        library_patch_id: "p2".into(),
        hash_at_transfer: "e-p2".into(),
        transferred_at: 2,
    })
    .unwrap();
    assert_eq!(s.links_for_device("SN1").len(), 1);
    assert_eq!(
        s.link_for_slot("SN1", &slot).unwrap().library_patch_id,
        "p2"
    );
    assert!(s.link_for_slot("SN2", &slot).is_none());

    // delete_group czyści przynależność na patchach
    s.delete_group(&"g1".into()).unwrap();
    assert!(s.get(&"p2".into()).unwrap().groups.is_empty());

    // remove patcha sprząta go z pozostałych grup
    s.create_group(Group {
        id: "g2".into(),
        name: "G2".into(),
        members: vec![],
    })
    .unwrap();
    s.add_to_group(&"g2".into(), &"p2".into()).unwrap();
    s.remove(&"p2".into()).unwrap();
    assert!(s.group(&"g2".into()).unwrap().members.is_empty());

    // indeks do sync po hashu
    let idx = s.index();
    assert_eq!(idx.find_exact(&"e-p1".into()), Some(&"p1".to_string()));
}

#[test]
fn memory_store_satisfies_contract() {
    let mut s = MemoryStore::new();
    contract_suite(&mut s);
}

#[test]
fn sqlite_store_satisfies_contract() {
    let mut s = SqliteStore::open_in_memory().unwrap();
    contract_suite(&mut s);
}

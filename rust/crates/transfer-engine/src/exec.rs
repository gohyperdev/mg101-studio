//! Wykonanie transferu (HLD §5 pkt 3-4). Zapis pod kontrolą: WAL (ADR-0001),
//! wykrycie konfliktu na slocie (`expectedRevision`), rollback całej partii przy
//! błędzie. Zależne tylko od traitów — testowane mockiem protokołu/transportu.

use mg101_core::wal::{EntryState, InverseOperation, JournalStore, TransactionEntry};
use mg101_device_link::DeviceLink;
use mg101_device_pack_api::{DeviceProtocol, ProtocolError, SlotAddr};
use mg101_library::{
    content_hash, exact_hash, DeviceSlotLink, LibraryError, LibraryPatch, LibraryStore, MaskRange,
    PatchOrigin,
};

use crate::plan::TransferPlan;

const TOOL_PUSH: &str = "push_slot";

/// Kontekst dostępu do urządzenia (protokół + transport).
pub struct SlotWriteContext<'a> {
    pub protocol: &'a dyn DeviceProtocol,
    pub link: &'a mut dyn DeviceLink,
}

/// Wynik udanego push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushOutcome {
    pub writes_applied: usize,
    pub links: Vec<DeviceSlotLink>,
}

/// Błąd wykonania transferu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// Patch z planu nie istnieje w Bibliotece.
    PatchMissing(String),
    /// Slot zmieniony poza aplikacją od ostatniego transferu (zapis wstrzymany).
    /// `rolled_back` = ile wcześniejszych zapisów partii cofnięto.
    Conflict {
        target: SlotAddr,
        rolled_back: usize,
    },
    /// Błąd protokołu urządzenia.
    Protocol(ProtocolError),
    /// Błąd dziennika WAL.
    Wal(String),
    /// Błąd składu Biblioteki (np. zapis linku provenance).
    Store(LibraryError),
}

fn slot_key(addr: &SlotAddr) -> String {
    format!("slot:{}:{}", addr.bank, addr.index)
}

/// Najlepszej-staranności cofnięcie już zapisanych slotów (odwrotna kolejność).
fn rollback(ctx: &mut SlotWriteContext, applied: &[(SlotAddr, Vec<u8>)]) {
    for (addr, before) in applied.iter().rev() {
        let _ = ctx.protocol.write_slot(ctx.link, addr, before);
    }
}

/// Wykonuje zatwierdzony plan push. Każdy zapis: odczyt „przed" (kontrola
/// konfliktu względem zapisanego linku), WAL prepared→committed, `write_slot`,
/// aktualizacja [`DeviceSlotLink`]. Błąd/konflikt w połowie → rollback partii.
///
/// `next_sequence`/`now_ms` dostarcza wywołujący (wasm-safe, deterministyczne
/// w testach). Plan MUSI pochodzić z [`crate::plan_push`] (walidacja całościowa).
pub fn execute_push<S: LibraryStore, J: JournalStore>(
    plan: &TransferPlan,
    ctx: &mut SlotWriteContext,
    store: &mut S,
    journal: &J,
    device_serial: &str,
    now_ms: i64,
    next_sequence: &mut u64,
) -> Result<PushOutcome, ExecError> {
    let mut applied: Vec<(SlotAddr, Vec<u8>)> = Vec::new();
    let mut links: Vec<DeviceSlotLink> = Vec::new();

    for w in &plan.writes {
        let Some(patch) = store.get(&w.patch_id) else {
            rollback(ctx, &applied);
            return Err(ExecError::PatchMissing(w.patch_id.clone()));
        };

        // Odczyt „przed" — hash bazowy (biblioteczny exact_hash: jedna przestrzeń
        // hashy, spójna z hash_at_transfer w linkach).
        let before = match ctx.protocol.read_slot(ctx.link, &w.target) {
            Ok(b) => b,
            Err(e) => {
                rollback(ctx, &applied);
                return Err(ExecError::Protocol(e));
            }
        };
        let before_hash = exact_hash(&before);

        // Kontrola konfliktu. Priorytet: jawny `expected_before_hash` z planu
        // (egzekwowany dla KAŻDEGO slotu); w jego braku — dawny link provenance.
        let expected = w.expected_before_hash.clone().or_else(|| {
            store
                .link_for_slot(device_serial, &w.target)
                .map(|l| l.hash_at_transfer)
        });
        if let Some(exp) = expected {
            if exp != before_hash {
                rollback(ctx, &applied);
                return Err(ExecError::Conflict {
                    target: w.target.clone(),
                    rolled_back: applied.len(),
                });
            }
        }

        // WAL: prepared (inverse = przywróć poprzednie bajty slotu).
        let seq = *next_sequence;
        *next_sequence += 1;
        let mut entry = TransactionEntry {
            sequence: seq,
            timestamp_ms: now_ms,
            tool_name: TOOL_PUSH.into(),
            patch_id: slot_key(&w.target),
            revision_before: patch.revision as i64,
            inverse: InverseOperation::RestoreBytes {
                patch_id: slot_key(&w.target),
                blob: before.clone(),
            },
            before_hash,
            after_hash: patch.exact_hash.clone(),
            state: EntryState::Prepared,
        };
        if let Err(e) = journal.append(&entry) {
            rollback(ctx, &applied);
            return Err(ExecError::Wal(e.to_string()));
        }

        // Zapis slotu.
        if let Err(e) = ctx.protocol.write_slot(ctx.link, &w.target, &patch.blob) {
            rollback(ctx, &applied);
            return Err(ExecError::Protocol(e));
        }
        applied.push((w.target.clone(), before)); // od teraz bieżący też podlega rollbackowi

        // WAL: committed. Błąd tu → rollback z bieżącym slotem włącznie.
        entry.state = EntryState::Committed;
        if let Err(e) = journal.append(&entry) {
            rollback(ctx, &applied);
            return Err(ExecError::Wal(e.to_string()));
        }

        // Aktualizacja provenance. Błąd → rollback z bieżącym slotem włącznie
        // (bez linku stan sync byłby fałszywy).
        let link = DeviceSlotLink {
            device_serial: device_serial.to_string(),
            slot: w.target.clone(),
            library_patch_id: patch.id.clone(),
            hash_at_transfer: patch.exact_hash.clone(),
            transferred_at: now_ms,
        };
        if let Err(e) = store.record_link(link.clone()) {
            rollback(ctx, &applied);
            return Err(ExecError::Store(e));
        }
        links.push(link);
    }

    Ok(PushOutcome {
        writes_applied: applied.len(),
        links,
    })
}

/// Import pojedynczego slotu do Biblioteki (Pull). Czyta blob z urządzenia i
/// buduje [`LibraryPatch`] (bajty święte + fingerprint). Wywołujący nadaje `id`
/// i dodaje wynik do store. `masks` (regiony nieistotne brzmieniowo) z profilu.
#[allow(clippy::too_many_arguments)]
pub fn pull_slot(
    ctx: &mut SlotWriteContext,
    addr: &SlotAddr,
    id: String,
    name: String,
    device_id: String,
    firmware: Option<String>,
    codec_version: String,
    masks: &[MaskRange],
    now_ms: i64,
) -> Result<LibraryPatch, ExecError> {
    let blob = ctx
        .protocol
        .read_slot(ctx.link, addr)
        .map_err(ExecError::Protocol)?;
    Ok(LibraryPatch {
        id,
        name,
        content_hash: content_hash(&blob, masks),
        exact_hash: exact_hash(&blob),
        blob,
        origin: PatchOrigin::PulledFromDevice,
        device_id,
        firmware,
        codec_version,
        tags: Default::default(),
        groups: Default::default(),
        created_at: now_ms,
        updated_at: now_ms,
        revision: 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{PlannedWrite, TransferPlan};
    use mg101_core::wal::WalError;
    use mg101_library::MemoryStore;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    // --- Dublury testowe ---

    /// Dziennik WAL w pamięci; opcjonalnie zawodzi na N-tym `append` (1-bazowo).
    #[derive(Default)]
    struct MemJournal {
        entries: RefCell<Vec<TransactionEntry>>,
        fail_append_at: Option<usize>,
        calls: RefCell<usize>,
    }
    impl JournalStore for MemJournal {
        fn append(&self, entry: &TransactionEntry) -> Result<(), WalError> {
            *self.calls.borrow_mut() += 1;
            if Some(*self.calls.borrow()) == self.fail_append_at {
                return Err(WalError::Io("symulowany błąd dziennika".into()));
            }
            self.entries.borrow_mut().push(entry.clone());
            Ok(())
        }
        fn load(&self) -> Result<Vec<TransactionEntry>, WalError> {
            Ok(self.entries.borrow().clone())
        }
        fn rewrite(&self, entries: &[TransactionEntry]) -> Result<(), WalError> {
            *self.entries.borrow_mut() = entries.to_vec();
            Ok(())
        }
    }

    /// Protokół w pamięci: mapa slot→blob; rejestruje zapisy; opcjonalny błąd zapisu.
    struct MockProtocol {
        slots: RefCell<BTreeMap<(String, u16), Vec<u8>>>,
        fail_write_at: Option<u16>, // indeks slotu, na którym write_slot zwróci błąd
    }
    impl MockProtocol {
        fn new() -> Self {
            Self {
                slots: RefCell::new(BTreeMap::new()),
                fail_write_at: None,
            }
        }
        fn seed(&self, bank: &str, index: u16, blob: Vec<u8>) {
            self.slots.borrow_mut().insert((bank.into(), index), blob);
        }
        fn read(&self, bank: &str, index: u16) -> Vec<u8> {
            self.slots
                .borrow()
                .get(&(bank.into(), index))
                .cloned()
                .unwrap_or_default()
        }
    }
    impl DeviceProtocol for MockProtocol {
        fn read_slot(
            &self,
            _link: &mut dyn DeviceLink,
            addr: &SlotAddr,
        ) -> Result<Vec<u8>, ProtocolError> {
            Ok(self.read(&addr.bank, addr.index))
        }
        fn write_slot(
            &self,
            _link: &mut dyn DeviceLink,
            addr: &SlotAddr,
            blob: &[u8],
        ) -> Result<(), ProtocolError> {
            if Some(addr.index) == self.fail_write_at {
                return Err(ProtocolError::BadResponse("symulowany błąd".into()));
            }
            self.slots
                .borrow_mut()
                .insert((addr.bank.clone(), addr.index), blob.to_vec());
            Ok(())
        }
    }

    fn dummy_link() -> mg101_device_link::MockLink {
        mg101_device_link::MockLink::new(|_| Vec::new())
    }

    fn lib_patch(id: &str, blob: Vec<u8>) -> LibraryPatch {
        LibraryPatch {
            id: id.into(),
            name: id.into(),
            content_hash: exact_hash(&blob),
            exact_hash: exact_hash(&blob),
            blob,
            origin: PatchOrigin::Created,
            device_id: "nux-mg101".into(),
            firmware: None,
            codec_version: "1".into(),
            tags: Default::default(),
            groups: Default::default(),
            created_at: 0,
            updated_at: 0,
            revision: 3,
        }
    }

    fn plan_of(bank: &str, items: &[(&str, u16, bool)]) -> TransferPlan {
        TransferPlan {
            bank: bank.into(),
            writes: items
                .iter()
                .map(|(id, idx, ow)| PlannedWrite {
                    patch_id: (*id).into(),
                    target: SlotAddr {
                        bank: bank.into(),
                        index: *idx,
                    },
                    overwrites: *ow,
                    expected_before_hash: None,
                })
                .collect(),
        }
    }

    #[test]
    fn push_writes_slots_records_links_and_wal() {
        let proto = MockProtocol::new();
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        store.add(lib_patch("p1", vec![2, 2, 2])).unwrap();
        let journal = MemJournal::default();
        let plan = plan_of("user", &[("p0", 0, false), ("p1", 1, false)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let out = execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 42, &mut seq).unwrap();

        assert_eq!(out.writes_applied, 2);
        assert_eq!(proto.read("user", 0), vec![1, 1, 1]);
        assert_eq!(proto.read("user", 1), vec![2, 2, 2]);
        // Link provenance zapisany dla obu slotów.
        assert_eq!(store.links_for_device("SN1").len(), 2);
        // WAL: 2× prepared + 2× committed.
        let entries = journal.load().unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].state, EntryState::Prepared);
        assert_eq!(entries[1].state, EntryState::Committed);
        assert_eq!(seq, 2);
    }

    #[test]
    fn conflict_when_slot_changed_since_last_transfer_aborts_and_rolls_back() {
        let proto = MockProtocol::new();
        proto.seed("user", 0, vec![9, 9]); // stan początkowy slotu 0
        proto.seed("user", 1, vec![8, 8]);
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        store.add(lib_patch("p1", vec![2, 2, 2])).unwrap();
        // Istniejący link dla slotu 1 z hashem NIEzgodnym z bieżącą zawartością.
        store
            .record_link(DeviceSlotLink {
                device_serial: "SN1".into(),
                slot: SlotAddr {
                    bank: "user".into(),
                    index: 1,
                },
                library_patch_id: "old".into(),
                hash_at_transfer: exact_hash(&[7, 7, 7]), // ≠ bieżące [8,8]
                transferred_at: 1,
            })
            .unwrap();
        let journal = MemJournal::default();
        // Plan pisze slot 0 (OK), potem slot 1 (konflikt).
        let plan = plan_of("user", &[("p0", 0, false), ("p1", 1, true)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let err =
            execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 1, &mut seq).unwrap_err();
        assert!(matches!(err, ExecError::Conflict { rolled_back: 1, .. }));
        // Rollback: slot 0 przywrócony do stanu początkowego.
        assert_eq!(proto.read("user", 0), vec![9, 9]);
        // Slot 1 nie został tknięty.
        assert_eq!(proto.read("user", 1), vec![8, 8]);
    }

    #[test]
    fn explicit_expected_hash_enables_acknowledged_overwrite() {
        // K2: nadpisanie stanu DeviceModified. Slot ma [8,8]; link mówił [7,7,7]
        // (rozjazd). Ustawiając expected_before_hash = bieżący hash [8,8], użytkownik
        // POTWIERDZA nadpisanie — zapis się wykonuje (a nie wieczny konflikt).
        let proto = MockProtocol::new();
        proto.seed("user", 0, vec![8, 8]);
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        store
            .record_link(DeviceSlotLink {
                device_serial: "SN1".into(),
                slot: SlotAddr {
                    bank: "user".into(),
                    index: 0,
                },
                library_patch_id: "old".into(),
                hash_at_transfer: exact_hash(&[7, 7, 7]),
                transferred_at: 1,
            })
            .unwrap();
        let journal = MemJournal::default();
        let mut plan = plan_of("user", &[("p0", 0, true)]);
        plan.writes[0].expected_before_hash = Some(exact_hash(&[8, 8])); // potwierdzenie
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let out = execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 5, &mut seq).unwrap();
        assert_eq!(out.writes_applied, 1);
        assert_eq!(proto.read("user", 0), vec![1, 1, 1]); // nadpisany mimo rozjazdu
    }

    #[test]
    fn wal_committed_append_error_rolls_back_current_slot() {
        // K1: błąd zapisu wpisu Committed (2. append) — slot już zapisany fizycznie,
        // MUSI zostać cofnięty (rollback obejmuje bieżący slot).
        let proto = MockProtocol::new();
        proto.seed("user", 0, vec![9, 9]);
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        let journal = MemJournal {
            fail_append_at: Some(2), // prepared OK, committed pada
            ..Default::default()
        };
        let plan = plan_of("user", &[("p0", 0, false)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let err =
            execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 1, &mut seq).unwrap_err();
        assert!(matches!(err, ExecError::Wal(_)));
        assert_eq!(proto.read("user", 0), vec![9, 9]); // cofnięty
        assert_eq!(store.links_for_device("SN1").len(), 0); // brak provenance
    }

    #[test]
    fn wal_prepared_append_error_rolls_back_prior_slots() {
        // K1: błąd zapisu Prepared dla 2. slotu — 1. slot (już zapisany) cofnięty.
        let proto = MockProtocol::new();
        proto.seed("user", 0, vec![9, 9]);
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        store.add(lib_patch("p1", vec![2, 2, 2])).unwrap();
        let journal = MemJournal {
            fail_append_at: Some(3), // slot0: prepared+committed OK; slot1: prepared pada
            ..Default::default()
        };
        let plan = plan_of("user", &[("p0", 0, false), ("p1", 1, false)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let err =
            execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 1, &mut seq).unwrap_err();
        assert!(matches!(err, ExecError::Wal(_)));
        assert_eq!(proto.read("user", 0), vec![9, 9]); // slot0 cofnięty
    }

    #[test]
    fn protocol_write_error_rolls_back_prior_writes() {
        let mut proto = MockProtocol::new();
        proto.seed("user", 0, vec![9, 9]);
        proto.fail_write_at = Some(1); // zapis slotu 1 się nie powiedzie
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        store.add(lib_patch("p0", vec![1, 1, 1])).unwrap();
        store.add(lib_patch("p1", vec![2, 2, 2])).unwrap();
        let journal = MemJournal::default();
        let plan = plan_of("user", &[("p0", 0, false), ("p1", 1, false)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let err =
            execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 1, &mut seq).unwrap_err();
        assert!(matches!(err, ExecError::Protocol(_)));
        // Slot 0 (już zapisany) cofnięty do stanu początkowego.
        assert_eq!(proto.read("user", 0), vec![9, 9]);
    }

    #[test]
    fn missing_patch_aborts() {
        let proto = MockProtocol::new();
        let mut link = dummy_link();
        let mut store = MemoryStore::new();
        let journal = MemJournal::default();
        let plan = plan_of("user", &[("ghost", 0, false)]);
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let mut seq = 0;
        let err =
            execute_push(&plan, &mut ctx, &mut store, &journal, "SN1", 1, &mut seq).unwrap_err();
        assert!(matches!(err, ExecError::PatchMissing(id) if id == "ghost"));
    }

    #[test]
    fn pull_slot_builds_library_patch_with_fingerprint() {
        let proto = MockProtocol::new();
        proto.seed("factory", 3, vec![10, b'N', b'A', b'M', b'E', 20]);
        let mut link = dummy_link();
        let mut ctx = SlotWriteContext {
            protocol: &proto,
            link: &mut link,
        };
        let masks = [MaskRange { start: 1, len: 4 }]; // region „nazwy"
        let p = pull_slot(
            &mut ctx,
            &SlotAddr {
                bank: "factory".into(),
                index: 3,
            },
            "id1".into(),
            "Imported".into(),
            "nux-mg101".into(),
            Some("1.0".into()),
            "1".into(),
            &masks,
            99,
        )
        .unwrap();
        assert_eq!(p.origin, PatchOrigin::PulledFromDevice);
        assert_eq!(p.blob, vec![10, b'N', b'A', b'M', b'E', 20]);
        assert_eq!(p.exact_hash, exact_hash(&p.blob));
        assert_eq!(p.content_hash, content_hash(&p.blob, &masks));
        assert_ne!(p.content_hash, p.exact_hash); // maska zmienia fingerprint
    }
}

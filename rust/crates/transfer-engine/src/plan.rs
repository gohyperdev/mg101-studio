//! Planowanie transferu (HLD §5) — **czysta logika**, bez we/wy ani sprzętu.
//!
//! Silnik zależy tylko od generycznej topologii ([`DeviceStorage`]) i danych
//! profilu (banki, limity, `writable`). Dla dowolnego urządzenia (128 slotów,
//! brak Factory itd.) ten sam kod działa bez zmian. Zasada bezpieczeństwa
//! zapisu z `mg101-probe`: **walidacja całościowa PRZED jakimkolwiek zapisem** —
//! Factory i przepełnienie odrzucamy na etapie planu, nie w połowie transferu.

use mg101_device_pack_api::{BankId, BankInfo, DeviceStorage, SlotAddr, SlotMap};
use mg101_library::PatchId;

/// Strategia rozmieszczenia patchy grupy w slotach banku (HLD §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Kolejne wolne sloty banku (w kolejności indeksów).
    NextFree,
    /// Sekwencyjnie od wskazanego indeksu (absolutnego, z `index_base`).
    /// Może nadpisać zajęte sloty (jawny wybór użytkownika).
    FromSlot(u16),
    /// Nadpisanie dokładnie zakresu `[start, start+len)` — liczba patchy musi
    /// równać się `len`.
    Overwrite { start: u16, len: u16 },
}

/// Zaplanowany pojedynczy zapis slotu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWrite {
    pub patch_id: PatchId,
    pub target: SlotAddr,
    /// Czy slot docelowy był zajęty (nadpisanie).
    pub overwrites: bool,
    /// Oczekiwany hash zawartości slotu PRZED zapisem (ze skanu w chwili planu).
    /// `execute_push` egzekwuje go dla KAŻDEGO slotu (nie tylko powiązanego):
    /// jeśli bieżąca zawartość ≠ oczekiwana → konflikt (ktoś zmienił slot między
    /// zatwierdzeniem planu a wykonaniem). `None` = pomiń kontrolę (świeży zapis
    /// do pustego slotu). Świadome nadpisanie stanu `DeviceModified` = ustawienie
    /// tu bieżącego hasha urządzenia (użytkownik potwierdza, że widzi ten stan).
    pub expected_before_hash: Option<String>,
}

/// Plan transferu grupowego — raport do zatwierdzenia / dry-run (HLD §5 pkt 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPlan {
    pub bank: BankId,
    pub writes: Vec<PlannedWrite>,
}

impl TransferPlan {
    /// Ile slotów zostanie nadpisanych (do ostrzeżenia w UI).
    pub fn overwrite_count(&self) -> usize {
        self.writes.iter().filter(|w| w.overwrites).count()
    }
}

/// Błąd planowania — zawsze przed zapisem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// Pusta grupa — nie ma czego transferować.
    EmptyGroup,
    /// Nieznany bank docelowy.
    BankNotFound(BankId),
    /// Bank tylko do odczytu (Factory).
    NotWritable(BankId),
    /// Za mało wolnych slotów dla strategii `NextFree`.
    NoFreeSlots { needed: usize, available: usize },
    /// Cel poza zakresem banku.
    OutOfRange(SlotAddr),
    /// Liczba patchy ≠ długość zakresu (`Overwrite`).
    CountMismatch { patches: usize, slots: usize },
    /// Przekroczony limit zapisywalnych slotów (profil).
    OverLimit {
        scope: String,
        limit: u16,
        resulting: u16,
    },
}

fn bank_info<'a>(storage: &'a dyn DeviceStorage, bank: &BankId) -> Option<&'a BankInfo> {
    storage.banks().iter().find(|b| &b.id == bank)
}

/// Planuje push grupy (uporządkowanej) do banku wg strategii. Waliduje
/// całościowo: Factory, zakres, liczność i limity — zanim cokolwiek zapisze.
pub fn plan_push(
    storage: &dyn DeviceStorage,
    bank: &BankId,
    occupied: &SlotMap,
    patches: &[PatchId],
    placement: Placement,
) -> Result<TransferPlan, PlanError> {
    if patches.is_empty() {
        return Err(PlanError::EmptyGroup);
    }
    let info = bank_info(storage, bank).ok_or_else(|| PlanError::BankNotFound(bank.clone()))?;
    if !info.writable {
        return Err(PlanError::NotWritable(bank.clone()));
    }
    let n = patches.len();

    // 1. Wyznacz indeksy docelowe (u32 — bez ryzyka przepełnienia u16 przy
    //    arytmetyce start+i; walidacja zakresu w kroku 2).
    let target_indices: Vec<u32> = match placement {
        Placement::NextFree => {
            let free = storage.free_slots(bank, occupied);
            if free.len() < n {
                return Err(PlanError::NoFreeSlots {
                    needed: n,
                    available: free.len(),
                });
            }
            free.into_iter().take(n).map(|a| a.index as u32).collect()
        }
        Placement::FromSlot(start) => (0..n).map(|i| start as u32 + i as u32).collect(),
        Placement::Overwrite { start, len } => {
            if n != len as usize {
                return Err(PlanError::CountMismatch {
                    patches: n,
                    slots: len as usize,
                });
            }
            (0..n).map(|i| start as u32 + i as u32).collect()
        }
    };

    // 2. Wszystkie cele muszą leżeć w zakresie banku (i mieścić się w u16).
    let base = info.index_base as u32;
    let max_index = base + info.slots as u32; // wyłącznie
    let mut targets: Vec<SlotAddr> = Vec::with_capacity(n);
    for idx in target_indices {
        if idx < base || idx >= max_index {
            return Err(PlanError::OutOfRange(SlotAddr {
                bank: bank.clone(),
                index: idx.min(u16::MAX as u32) as u16,
            }));
        }
        targets.push(SlotAddr {
            bank: bank.clone(),
            index: idx as u16, // < max_index ≤ u16 (slots/index_base to u16)
        });
    }

    // 3. Limity (dane profilu) — liczone na stanie WYNIKOWYM (po zapisie).
    let mut resulting = occupied.clone();
    for t in &targets {
        resulting.insert(t.clone());
    }
    let writable_banks: std::collections::HashSet<&BankId> = storage
        .banks()
        .iter()
        .filter(|b| b.writable)
        .map(|b| &b.id)
        .collect();
    let limits = storage.limits();
    // Limit całkowity (suma zajętych slotów zapisywalnych banków).
    if let Some(total) = limits.max_writable_total {
        let count = resulting
            .iter()
            .filter(|a| writable_banks.contains(&a.bank))
            .count() as u16;
        if count > total {
            return Err(PlanError::OverLimit {
                scope: "total".into(),
                limit: total,
                resulting: count,
            });
        }
    }
    // Limit per bank docelowy.
    if let Some(per) = limits.per_bank.get(bank) {
        let count = resulting.iter().filter(|a| &a.bank == bank).count() as u16;
        if count > *per {
            return Err(PlanError::OverLimit {
                scope: bank.clone(),
                limit: *per,
                resulting: count,
            });
        }
    }

    // 4. Zbuduj plan (nadpisania oznaczone względem stanu WEJŚCIOWEGO).
    //    `expected_before_hash` domyślnie None — wypełnia je warstwa sync ze
    //    skanu slotów (kontrola konfliktu przy wykonaniu).
    let writes = patches
        .iter()
        .zip(targets)
        .map(|(patch_id, target)| PlannedWrite {
            overwrites: occupied.contains(&target),
            patch_id: patch_id.clone(),
            target,
            expected_before_hash: None,
        })
        .collect();

    Ok(TransferPlan {
        bank: bank.clone(),
        writes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mg101_device_pack_api::StorageLimits;
    use std::collections::BTreeMap;

    struct TestStorage {
        banks: Vec<BankInfo>,
        limits: StorageLimits,
    }
    impl DeviceStorage for TestStorage {
        fn banks(&self) -> &[BankInfo] {
            &self.banks
        }
        fn limits(&self) -> &StorageLimits {
            &self.limits
        }
    }

    fn mg101_like() -> TestStorage {
        TestStorage {
            banks: vec![
                BankInfo {
                    id: "user".into(),
                    label: "User".into(),
                    slots: 36,
                    index_base: 0,
                    writable: true,
                    reorderable: true,
                    erasable: false,
                },
                BankInfo {
                    id: "factory".into(),
                    label: "Factory".into(),
                    slots: 36,
                    index_base: 0,
                    writable: false,
                    reorderable: false,
                    erasable: false,
                },
            ],
            limits: StorageLimits {
                max_writable_total: Some(36),
                per_bank: BTreeMap::from([("user".to_string(), 36)]),
            },
        }
    }

    fn ids(n: usize) -> Vec<PatchId> {
        (0..n).map(|i| format!("p{i}")).collect()
    }

    #[test]
    fn rejects_empty_group() {
        let s = mg101_like();
        assert_eq!(
            plan_push(
                &s,
                &"user".into(),
                &SlotMap::new(),
                &[],
                Placement::NextFree
            ),
            Err(PlanError::EmptyGroup)
        );
    }

    #[test]
    fn rejects_factory_before_any_write() {
        let s = mg101_like();
        assert_eq!(
            plan_push(
                &s,
                &"factory".into(),
                &SlotMap::new(),
                &ids(1),
                Placement::NextFree
            ),
            Err(PlanError::NotWritable("factory".into()))
        );
    }

    #[test]
    fn next_free_fills_lowest_free_slots() {
        let s = mg101_like();
        let mut occ = SlotMap::new();
        occ.insert(SlotAddr {
            bank: "user".into(),
            index: 0,
        }); // slot 0 zajęty
        let plan = plan_push(&s, &"user".into(), &occ, &ids(2), Placement::NextFree).unwrap();
        assert_eq!(plan.writes[0].target.index, 1);
        assert_eq!(plan.writes[1].target.index, 2);
        assert_eq!(plan.overwrite_count(), 0);
    }

    #[test]
    fn next_free_rejects_when_insufficient_space() {
        let s = mg101_like();
        // Zapełnij 35 z 36 slotów.
        let occ: SlotMap = (0..35)
            .map(|i| SlotAddr {
                bank: "user".into(),
                index: i,
            })
            .collect();
        let err = plan_push(&s, &"user".into(), &occ, &ids(2), Placement::NextFree).unwrap_err();
        assert!(matches!(
            err,
            PlanError::NoFreeSlots {
                needed: 2,
                available: 1
            }
        ));
    }

    #[test]
    fn from_slot_marks_overwrites_and_range_checks() {
        let s = mg101_like();
        let mut occ = SlotMap::new();
        occ.insert(SlotAddr {
            bank: "user".into(),
            index: 5,
        });
        let plan = plan_push(&s, &"user".into(), &occ, &ids(2), Placement::FromSlot(5)).unwrap();
        assert_eq!(plan.writes[0].target.index, 5);
        assert!(plan.writes[0].overwrites); // slot 5 był zajęty
        assert!(!plan.writes[1].overwrites);
        // Poza zakresem banku.
        let err =
            plan_push(&s, &"user".into(), &occ, &ids(2), Placement::FromSlot(35)).unwrap_err();
        assert!(matches!(err, PlanError::OutOfRange(a) if a.index == 36));
    }

    #[test]
    fn overwrite_requires_matching_count() {
        let s = mg101_like();
        let err = plan_push(
            &s,
            &"user".into(),
            &SlotMap::new(),
            &ids(3),
            Placement::Overwrite { start: 0, len: 5 },
        )
        .unwrap_err();
        assert!(matches!(
            err,
            PlanError::CountMismatch {
                patches: 3,
                slots: 5
            }
        ));
    }

    /// Urządzenie z jednym bankiem 128 slotów, ale limitem total 100 (HLD §5:
    /// „128 slotów, limit 100" — pojemność > limit).
    fn flat_128_limit_100() -> TestStorage {
        TestStorage {
            banks: vec![BankInfo {
                id: "main".into(),
                label: "Main".into(),
                slots: 128,
                index_base: 0,
                writable: true,
                reorderable: true,
                erasable: true,
            }],
            limits: StorageLimits {
                max_writable_total: Some(100),
                per_bank: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn enforces_total_limit_when_capacity_exceeds_limit() {
        // 100 slotów zajętych; dołożenie 1 do wolnego slotu 100 → wynik 101 > 100.
        let s = flat_128_limit_100();
        let occ: SlotMap = (0..100)
            .map(|i| SlotAddr {
                bank: "main".into(),
                index: i,
            })
            .collect();
        let err =
            plan_push(&s, &"main".into(), &occ, &ids(1), Placement::FromSlot(100)).unwrap_err();
        assert!(matches!(
            err,
            PlanError::OverLimit {
                limit: 100,
                resulting: 101,
                ..
            }
        ));
        // Ten sam slot 100 nadpisany (gdyby zajęty) nie łamie limitu — tu wolny,
        // więc dokładnie limit jest bramką, nie pojemność (jest jeszcze 28 slotów).
    }

    #[test]
    fn respects_index_base_one() {
        let s = TestStorage {
            banks: vec![BankInfo {
                id: "a".into(),
                label: "A".into(),
                slots: 4,
                index_base: 1, // sloty 1..5
                writable: true,
                reorderable: true,
                erasable: true,
            }],
            limits: StorageLimits::default(),
        };
        // FromSlot(1) mieści się (1,2), FromSlot(0) i FromSlot(4)+2 wypadają.
        let plan = plan_push(
            &s,
            &"a".into(),
            &SlotMap::new(),
            &ids(2),
            Placement::FromSlot(1),
        )
        .unwrap();
        assert_eq!(plan.writes[0].target.index, 1);
        assert!(matches!(
            plan_push(&s, &"a".into(), &SlotMap::new(), &ids(1), Placement::FromSlot(0)),
            Err(PlanError::OutOfRange(a)) if a.index == 0
        ));
        assert!(matches!(
            plan_push(&s, &"a".into(), &SlotMap::new(), &ids(2), Placement::FromSlot(4)),
            Err(PlanError::OutOfRange(a)) if a.index == 5
        ));
        // free_slots też respektuje bazę: 4 wolne od indeksu 1.
        let plan = plan_push(
            &s,
            &"a".into(),
            &SlotMap::new(),
            &ids(1),
            Placement::NextFree,
        )
        .unwrap();
        assert_eq!(plan.writes[0].target.index, 1);
    }

    #[test]
    fn u16_overflow_in_from_slot_is_out_of_range_not_panic() {
        // start blisko granicy u16: brak paniki (arytmetyka u32), zwraca OutOfRange.
        let s = mg101_like();
        let err = plan_push(
            &s,
            &"user".into(),
            &SlotMap::new(),
            &ids(2),
            Placement::FromSlot(u16::MAX),
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::OutOfRange(_)));
    }

    #[test]
    fn per_bank_limit_smaller_than_capacity() {
        // Bank ma 36 slotów, ale limit per_bank = 4.
        let mut s = mg101_like();
        s.limits.per_bank.insert("user".into(), 4);
        s.limits.max_writable_total = Some(100);
        let occ: SlotMap = (0..4)
            .map(|i| SlotAddr {
                bank: "user".into(),
                index: i,
            })
            .collect();
        let err = plan_push(&s, &"user".into(), &occ, &ids(1), Placement::FromSlot(4)).unwrap_err();
        assert!(matches!(
            err,
            PlanError::OverLimit {
                limit: 4,
                resulting: 5,
                ..
            }
        ));
    }

    #[test]
    fn overwriting_existing_does_not_exceed_limit() {
        // Nadpisanie zajętego slotu nie zwiększa liczby zajętych → limit OK.
        let mut s = mg101_like();
        s.limits.per_bank.insert("user".into(), 4);
        let occ: SlotMap = (0..4)
            .map(|i| SlotAddr {
                bank: "user".into(),
                index: i,
            })
            .collect();
        let plan = plan_push(&s, &"user".into(), &occ, &ids(2), Placement::FromSlot(0)).unwrap();
        assert_eq!(plan.overwrite_count(), 2); // sloty 0,1 nadpisane, brak wzrostu
    }
}

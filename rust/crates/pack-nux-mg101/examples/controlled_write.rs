//! Dowód #5 (część zapisu): **kontrolowany, idempotentny** zapis na żywym MG-101
//! z dziennikiem WAL (plan → prepared → write → verify → committed).
//!
//! BEZPIECZEŃSTWO: zapisuje slotowi User 0 **jego własne, właśnie odczytane
//! bajty** — urządzenie nie zmienia się netto. Uruchamia się WYŁĄCZNIE z jawną
//! zgodą przez zmienną środowiskową, by nigdy nie odpalić przypadkiem:
//!
//!   MG101_ALLOW_WRITE=1 cargo run -p mg101-pack-nux-mg101 --example controlled_write
//!
//! Plan: [write user/0 ← bajty(user/0)]. WAL: wpis `Prepared` (inverse =
//! przywróć te same bajty) → zapis → ponowny odczyt i porównanie → `Committed`.
//! Gdyby weryfikacja zawiodła, inverse w dzienniku pozwala przywrócić stan.

use mg101_core::wal::{sha256_hex, EntryState, InverseOperation, TransactionEntry};
use mg101_device_link::MidirLink;
use mg101_device_pack_api::{DeviceProtocol, SlotAddr};
use mg101_pack_nux_mg101::Mg101Protocol;

fn main() {
    if std::env::var("MG101_ALLOW_WRITE").as_deref() != Ok("1") {
        eprintln!(
            "Odmowa: kontrolowany zapis na sprzęt wymaga jawnej zgody.\n\
             Uruchom: MG101_ALLOW_WRITE=1 cargo run -p mg101-pack-nux-mg101 --example controlled_write"
        );
        std::process::exit(2);
    }

    let mut link = match MidirLink::open("MG-101") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Nie otwarto łącza do MG-101: {e}");
            std::process::exit(1);
        }
    };
    let addr = SlotAddr {
        bank: "user".into(),
        index: 0,
    };

    // 1) Odczyt bieżących bajtów slotu (to je zapiszemy z powrotem — idempotencja).
    let original = match Mg101Protocol.read_slot(&mut link, &addr) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Odczyt slotu User 0 nieudany: {e:?}");
            std::process::exit(1);
        }
    };
    let before_hash = sha256_hex(&original);
    println!(
        "Plan: zapis user/0 ← {} B (idempotentnie, hash {}…).",
        original.len(),
        &before_hash[..12]
    );

    // 2) WAL: wpis Prepared. Inverse = zapisać te same oryginalne bajty (przy
    //    idempotentnym zapisie to no-op, ale dziennik jest kompletny na wypadek awarii).
    let mut journal: Vec<TransactionEntry> = Vec::new();
    let entry = TransactionEntry {
        sequence: 0,
        timestamp_ms: 0,
        tool_name: "device_write".into(),
        patch_id: "slot:user:0".into(),
        revision_before: 0,
        inverse: InverseOperation::RestoreBytes {
            patch_id: "slot:user:0".into(),
            blob: original.clone(),
        },
        before_hash: before_hash.clone(),
        after_hash: before_hash.clone(), // idempotentnie: po == przed
        state: EntryState::Prepared,
    };
    journal.push(entry.clone());
    println!("WAL: wpis Prepared (seq=0, inverse=RestoreBytes user/0).");

    // 3) Zapis (walidowany: 189 B, 7-bitowy — patrz write_slot).
    if let Err(e) = Mg101Protocol.write_slot(&mut link, &addr, &original) {
        eprintln!("Zapis nieudany (dziennik pozostaje Prepared): {e:?}");
        std::process::exit(1);
    }
    println!("Zapis wysłany na urządzenie (SUB 01, 189 B).");

    // 4) Weryfikacja: ponowny odczyt musi dać identyczne bajty.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let after = match Mg101Protocol.read_slot(&mut link, &addr) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Odczyt weryfikacyjny nieudany: {e:?}");
            std::process::exit(1);
        }
    };
    let after_hash = sha256_hex(&after);
    if after != original {
        eprintln!(
            "WERYFIKACJA NIEUDANA: slot zmienił się (przed {}… po {}…). \
             Inverse w WAL (RestoreBytes) przywróciłby oryginał.",
            &before_hash[..12],
            &after_hash[..12]
        );
        std::process::exit(1);
    }

    // 5) WAL: Committed.
    if let Some(e) = journal.last_mut() {
        e.state = EntryState::Committed;
    }
    println!(
        "Weryfikacja OK: slot user/0 identyczny po zapisie ({} B, hash {}…).",
        after.len(),
        &after_hash[..12]
    );
    println!("WAL: wpis Committed. Kontrolowany zapis (WAL+plan) POTWIERDZONY na żywym MG-101.");
}

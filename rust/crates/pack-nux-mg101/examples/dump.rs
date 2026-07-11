//! Narzędzie E2: read-only zrzut banku User z fizycznego MG-101.
//!
//! Uruchomienie (sprzęt podłączony i zasilony):
//!   cargo run -p mg101-pack-nux-mg101 --example dump
//!
//! WYŁĄCZNIE odczyt (ramki żądania `SUB 00`). Nie wysyła żadnych zapisów.
//! Wypisuje dla każdego z 36 slotów User długość i skrót payloadu (189 B wire).

use mg101_device_link::MidirLink;
use mg101_device_pack_api::{BankInfo, DeviceProtocol, SlotAddr};
use mg101_pack_nux_mg101::Mg101Protocol;

fn main() {
    let needle = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "MG-101".to_string());
    let mut link = match MidirLink::open(&needle) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Nie otwarto łącza do '{needle}': {e}");
            eprintln!("Podłącz i zasil MG-101, sprawdź nazwę portu MIDI.");
            std::process::exit(1);
        }
    };
    println!("Łącze otwarte ('{needle}'). Read-only zrzut banku User (36 slotów).");

    let user = BankInfo {
        id: "user".into(),
        label: "User".into(),
        slots: 36,
        index_base: 0,
        writable: true,
        reorderable: true,
        erasable: false,
    };

    let mut ok = 0usize;
    for i in 0..user.slots {
        let addr = SlotAddr {
            bank: "user".into(),
            index: i,
        };
        match Mg101Protocol.read_slot(&mut link, &addr) {
            Ok(payload) => {
                ok += 1;
                let head: String = payload
                    .iter()
                    .take(16)
                    .map(|b| format!("{b:02X} "))
                    .collect();
                println!("slot {i:2}: {} B | {head}...", payload.len());
            }
            Err(e) => println!("slot {i:2}: BŁĄD {e:?}"),
        }
    }
    println!("Odczytano {ok}/36 slotów User.");
}

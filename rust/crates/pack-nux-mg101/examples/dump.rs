//! Narzędzie E2: read-only zrzut całego banku (User + Factory = 72 sloty) z
//! fizycznego MG-101, z kontrolą wierności i zapisem surowego zrzutu.
//!
//! Uruchomienie (sprzęt podłączony i zasilony):
//!   cargo run -p mg101-pack-nux-mg101 --example dump [ścieżka_wyjściowa]
//!
//! WYŁĄCZNIE odczyt (ramki żądania `SUB 00`). Nie wysyła żadnych zapisów.
//! Kontrola wierności: slot 0 czytany dwukrotnie musi dać identyczne bajty.

use mg101_device_link::MidirLink;
use mg101_device_pack_api::{DeviceProtocol, SlotAddr};
use mg101_pack_nux_mg101::Mg101Protocol;

const BANKS: [(&str, u16); 2] = [("user", 36), ("factory", 36)];

fn read(link: &mut MidirLink, bank: &str, index: u16) -> Result<Vec<u8>, String> {
    Mg101Protocol
        .read_slot(
            link,
            &SlotAddr {
                bank: bank.into(),
                index,
            },
        )
        .map_err(|e| format!("{e:?}"))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out_path = args.next();
    let needle = "MG-101";

    let mut link = match MidirLink::open(needle) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Nie otwarto łącza do '{needle}': {e}");
            eprintln!("Podłącz i zasil MG-101, sprawdź nazwę portu MIDI.");
            std::process::exit(1);
        }
    };
    println!("Łącze otwarte. Read-only zrzut 72 slotów (User + Factory).");

    // Kontrola wierności: dwa odczyty slotu 0 muszą być identyczne.
    match (read(&mut link, "user", 0), read(&mut link, "user", 0)) {
        (Ok(a), Ok(b)) if a == b => {
            println!(
                "Wierność: dwukrotny odczyt slotu User 0 identyczny ({} B). OK",
                a.len()
            )
        }
        (Ok(a), Ok(b)) => {
            eprintln!("Wierność ZAWIODŁA: {} B vs {} B (różne)", a.len(), b.len());
            std::process::exit(2);
        }
        _ => {
            eprintln!("Wierność: brak odpowiedzi na slot User 0");
            std::process::exit(2);
        }
    }

    let mut dump: Vec<u8> = Vec::new();
    let mut ok = 0usize;
    let mut lens = std::collections::BTreeSet::new();
    for (bank, count) in BANKS {
        for i in 0..count {
            match read(&mut link, bank, i) {
                Ok(payload) => {
                    ok += 1;
                    lens.insert(payload.len());
                    // Zapis: 2 bajty długości (LE) + payload — samoopisujący blok.
                    dump.extend_from_slice(&(payload.len() as u16).to_le_bytes());
                    dump.extend_from_slice(&payload);
                }
                Err(e) => println!("{bank} {i:2}: BŁĄD {e}"),
            }
        }
    }
    println!("Odczytano {ok}/72 slotów. Długości payloadów: {lens:?}");

    if let Some(path) = out_path {
        match std::fs::write(&path, &dump) {
            Ok(()) => println!("Surowy zrzut zapisany: {path} ({} B)", dump.len()),
            Err(e) => eprintln!("Nie zapisano {path}: {e}"),
        }
    }
}

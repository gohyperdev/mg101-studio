//! Monitor MIDI (reverse ustawień centralnych, np. EXP / drum / loop). Otwiera
//! port wykrytego urządzenia i wypisuje KAŻDY komunikat przychodzący, poprawnie
//! poramkowany (SysEx sklejony, komunikaty kanałowe wg długości statusu).
//!
//! Użycie: podłącz MG-101, uruchom, potem na URZĄDZENIU przełączaj dane
//! ustawienie (np. guzik EXP przez wszystkie wartości WAH→EFX→AMP→…). Każde
//! naciśnięcie powinno wygenerować komunikat — skopiuj log i wklej do rozmowy.
//!
//!   cargo run -p mg101-desktop --example midi_monitor
//!
//! Read-only: nic nie wysyłamy do urządzenia.

use mg101_desktop::device::detect;
use mg101_device_link::{DeviceLink, MidirLink, SysexAssembler};

fn describe(msg: &[u8]) -> String {
    let hex: Vec<String> = msg.iter().map(|b| format!("{b:02X}")).collect();
    let hex = hex.join(" ");
    let tag = match msg.first() {
        Some(&s) if (0xB0..=0xBF).contains(&s) && msg.len() >= 3 => {
            format!("CC  kanał {} | CC{} = {}", s & 0x0F, msg[1], msg[2])
        }
        Some(&s) if (0xC0..=0xCF).contains(&s) && msg.len() >= 2 => {
            format!("PC  kanał {} | program {}", s & 0x0F, msg[1])
        }
        Some(0xF0) => format!("SysEx ({} B)", msg.len()),
        _ => "inny".to_string(),
    };
    format!("{tag:32}  [{hex}]")
}

fn main() {
    let Some(dev) = detect() else {
        eprintln!("Brak urządzenia (żaden port MIDI nie pasuje do rejestru).");
        std::process::exit(1);
    };
    println!(
        "Monitor MIDI: {} {} (port ~'{}'). Przełączaj ustawienie na urządzeniu…\n",
        dev.manufacturer, dev.model, dev.port_needle
    );
    let mut link = match MidirLink::open(dev.port_needle) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Nie otwarto łącza: {e}");
            std::process::exit(2);
        }
    };
    let mut asm = SysexAssembler::new();
    let mut n = 0usize;
    loop {
        for raw in link.poll() {
            for msg in asm.push(&raw) {
                // Pomijamy realtime clock (F8) — zaśmieca log.
                if msg == [0xF8] {
                    continue;
                }
                n += 1;
                println!("{n:>4}  {}", describe(&msg));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

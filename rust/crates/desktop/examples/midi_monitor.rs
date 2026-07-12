//! Monitor MIDI — DIAGNOSTYKA. Otwiera WSZYSTKIE porty wejściowe MIDI (nie tylko
//! MG-101) i wypisuje KAŻDY komunikat SUROWO (bez asemblera SysEx, żeby nic nie
//! zgubić — pedał EXP nadaje długi SysEx `14`, który reasembler potrafi odrzucić).
//! Każda linia ma nazwę portu — jeśli pedał jest na innym endpoincie, zobaczymy go.
//!
//! Użycie: podłącz MG-101, uruchom, rusz pedał/kontroler, skopiuj log:
//!   cargo run -p mg101-desktop --example midi_monitor
//!
//! Read-only: nic nie wysyłamy.

use std::sync::mpsc;

use midir::{Ignore, MidiInput};

fn describe(port: &str, msg: &[u8]) -> String {
    let hex: Vec<String> = msg.iter().map(|b| format!("{b:02X}")).collect();
    let tag = match msg.first() {
        Some(&s) if (0xB0..=0xBF).contains(&s) && msg.len() >= 3 => {
            format!("CC{}={}", msg[1], msg[2])
        }
        Some(&s) if (0xC0..=0xCF).contains(&s) && msg.len() >= 2 => format!("PC {}", msg[1]),
        Some(0xF0) => format!("SysEx {}B", msg.len()),
        Some(&s) if (0xF0..=0xFF).contains(&s) => "sys".to_string(),
        _ => "?".to_string(),
    };
    format!("[{port:<18}] {tag:10}  {}", hex.join(" "))
}

fn main() {
    let scan = MidiInput::new("mg101-scan").expect("MidiInput");
    let ports = scan.ports();
    if ports.is_empty() {
        eprintln!("Brak portów wejściowych MIDI.");
        std::process::exit(1);
    }
    println!("Porty wejściowe MIDI ({}):", ports.len());
    for p in &ports {
        println!("  - {}", scan.port_name(p).unwrap_or_else(|_| "?".into()));
    }
    println!("\nNasłuch na WSZYSTKICH portach. Rusz pedał/kontroler…\n");

    let (tx, rx) = mpsc::channel::<(String, Vec<u8>)>();
    // Trzymamy połączenia przy życiu do końca programu.
    let mut conns = Vec::new();
    for p in ports {
        let mut input = MidiInput::new("mg101-mon").expect("MidiInput");
        input.ignore(Ignore::None); // NIE filtruj SysEx/aktywnych/czasu.
        let name = input.port_name(&p).unwrap_or_else(|_| "?".into());
        let txc = tx.clone();
        let nm = name.clone();
        match input.connect(
            &p,
            "mon",
            move |_ts, bytes: &[u8], _: &mut ()| {
                let _ = txc.send((nm.clone(), bytes.to_vec()));
            },
            (),
        ) {
            Ok(c) => conns.push(c),
            Err(e) => eprintln!("Nie podłączono portu {name}: {e}"),
        }
    }
    if conns.is_empty() {
        eprintln!("Nie udało się podłączyć żadnego portu.");
        std::process::exit(2);
    }

    let mut n = 0usize;
    for (port, msg) in rx {
        // Pomijamy realtime clock (F8) i active sensing (FE) — zaśmiecają.
        if msg == [0xF8] || msg == [0xFE] {
            continue;
        }
        n += 1;
        println!("{n:>5}  {}", describe(&port, &msg));
    }
}

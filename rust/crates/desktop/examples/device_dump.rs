//! Diagnostyka ścieżki importu z urządzenia (W2/W6) — ten sam kod co przycisk
//! „Połącz i zrzuć" w UI, ale bez okna. Read-only. Uruchom z podłączonym MG-101:
//!   cargo run -p mg101-desktop --example device_dump

use mg101_desktop::device::{detect, DumpMsg, Dumper};

fn main() {
    match detect() {
        Some(d) => println!(
            "Wykryto: {} {} (port ~'{}')",
            d.manufacturer, d.model, d.port_needle
        ),
        None => {
            eprintln!("Brak urządzenia (żaden port MIDI nie pasuje do rejestru).");
            std::process::exit(1);
        }
    }
    let dev = detect().unwrap();
    let dumper = Dumper::start(dev.port_needle, dev.slots_per_bank);
    loop {
        for m in dumper.poll() {
            match m {
                DumpMsg::Progress { done, total } => {
                    if done == total || done % 12 == 0 {
                        println!("  postęp {done}/{total}");
                    }
                }
                DumpMsg::Done { user, factory } => {
                    let occ = |v: &[mg101_desktop::device::SlotDump]| {
                        v.iter().filter(|s| s.occupied()).count()
                    };
                    println!(
                        "GOTOWE: User {} slotów ({} zajętych), Factory {} slotów ({} zajętych)",
                        user.len(),
                        occ(&user),
                        factory.len(),
                        occ(&factory),
                    );
                    if let Some(s) = user.first() {
                        println!(
                            "  User[0] blob {} B, pierwsze 8: {:02X?}",
                            s.blob.len(),
                            &s.blob[..8.min(s.blob.len())]
                        );
                    }
                    return;
                }
                DumpMsg::Error(e) => {
                    eprintln!("BŁĄD: {e}");
                    std::process::exit(2);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

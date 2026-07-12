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
                    // Dekodowanie wire→plik: nazwy slotów + parametry pierwszego patcha.
                    use mg101_pack_nux_mg101::wire;
                    println!("  Nazwy Factory 0..8 (dekod wire):");
                    for s in factory.iter().take(8) {
                        println!("    {:02}: {}", s.index, wire::decode_name(&s.blob));
                    }
                    if let Some(s) = factory.first() {
                        if let Ok(rec) = wire::decode_slot(&s.blob) {
                            let (profile, catalog) = mg101_pack_nux_mg101::load().unwrap();
                            let pr = mg101_core::PatchRecord::new(rec, &profile).unwrap();
                            println!(
                                "  Factory[0] '{}' BPM={} — modele bloków:",
                                pr.name(),
                                pr.bpm()
                            );
                            for b in &profile.blocks {
                                let id = pr.model_id(b);
                                let name = catalog
                                    .model(&b.id, id)
                                    .map(|m| m.display_name.as_str())
                                    .unwrap_or("—");
                                println!("    {:>4} = {} (model {id})", b.id, name);
                            }
                        }
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

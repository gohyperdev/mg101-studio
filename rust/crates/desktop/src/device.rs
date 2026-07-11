//! Warstwa urządzenia dla powłoki desktop: rejestr packów (W6) + zrzut banków
//! z fizycznego MG-101 w tle (W2). Natywna (MIDI przez `midir`, nie na wasm).
//!
//! Zrzut jest blokujący (72 sloty × round-trip SysEx ≈ sekundy), więc biegnie na
//! osobnym wątku; wyniki wracają kanałem, a wątek UI odpytuje [`Dumper::poll`]
//! w istniejącym Timerze. `Studio` pozostaje jednowątkowe.
//!
//! UWAGA: rekord slotu z urządzenia to forma **wire** (189 B), inna niż plik
//! `.mg101patch` (8402 B). Zrzut pokazuje zajętość slotów; pełne mapowanie na
//! parametry wymaga dekodera wire↔plik (BACKLOG W2).

use std::sync::mpsc::Receiver;
use std::thread;

use mg101_device_link::{input_port_names, MidirLink};
use mg101_device_pack_api::{DeviceProtocol, SlotAddr};
use mg101_pack_nux_mg101::Mg101Protocol;

/// Opis wspieranego urządzenia (pozycja rejestru). Przygotowanie pod kolejne
/// urządzenia: dobór packa (profil+katalog) i sposób wykrycia po nazwie portu.
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    /// Stabilny identyfikator packa (np. `nux.mg101`).
    pub id: &'static str,
    pub manufacturer: &'static str,
    pub model: &'static str,
    /// Fragment nazwy portu MIDI identyfikujący urządzenie (auto-detekcja).
    pub port_needle: &'static str,
    /// Liczba slotów per bank (User/Factory).
    pub slots_per_bank: u16,
}

/// Rejestr wspieranych urządzeń. Dziś jeden wpis (MG-101); struktura pod kolejne.
pub fn registry() -> &'static [DeviceDescriptor] {
    &[DeviceDescriptor {
        id: "nux.mg101",
        manufacturer: "NUX",
        model: "MG-101",
        port_needle: "MG-101",
        slots_per_bank: 36,
    }]
}

/// Wykrywa podłączone urządzenie po nazwach portów MIDI (W6 auto-detekcja).
/// Zwraca pierwszy pack z rejestru, którego `port_needle` pasuje do portu.
pub fn detect() -> Option<&'static DeviceDescriptor> {
    let ports = input_port_names();
    registry()
        .iter()
        .find(|d| ports.iter().any(|p| p.contains(d.port_needle)))
}

/// Pojedynczy zrzucony slot (surowy blob wire + metadane widoku).
#[derive(Debug, Clone)]
pub struct SlotDump {
    pub bank: String,
    pub index: u16,
    pub blob: Vec<u8>,
}

impl SlotDump {
    /// Slot zajęty = blob niepusty i nie same zera (heurystyka do UI).
    pub fn occupied(&self) -> bool {
        self.blob.iter().any(|&b| b != 0)
    }
}

/// Zdarzenie postępu/wyniku zrzutu (wątek tła → UI).
#[derive(Debug, Clone)]
pub enum DumpMsg {
    /// Odczytano `done` z `total` slotów.
    Progress { done: u16, total: u16 },
    /// Zrzut zakończony: sloty User i Factory.
    Done {
        user: Vec<SlotDump>,
        factory: Vec<SlotDump>,
    },
    /// Błąd (np. urządzenie odłączone) — zlokalizowany komunikat.
    Error(String),
}

/// Uchwyt zrzutu w tle. Wątek żyje do końca zrzutu; kanał niesie postęp/wynik.
pub struct Dumper {
    rx: Receiver<DumpMsg>,
}

impl Dumper {
    /// Startuje zrzut obu banków (`slots_per_bank` × 2) z portu `needle`.
    pub fn start(needle: &'static str, slots_per_bank: u16) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let mut link = match MidirLink::open(needle) {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx.send(DumpMsg::Error(format!("otwarcie łącza: {e}")));
                    return;
                }
            };
            let total = slots_per_bank * 2;
            let mut done = 0u16;
            let mut banks: [(&str, Vec<SlotDump>); 2] =
                [("user", Vec::new()), ("factory", Vec::new())];
            for (bank, out) in banks.iter_mut() {
                for index in 0..slots_per_bank {
                    let addr = SlotAddr {
                        bank: (*bank).into(),
                        index,
                    };
                    match Mg101Protocol.read_slot(&mut link, &addr) {
                        Ok(blob) => out.push(SlotDump {
                            bank: (*bank).into(),
                            index,
                            blob,
                        }),
                        Err(e) => {
                            let _ =
                                tx.send(DumpMsg::Error(format!("odczyt {bank}/{index}: {e:?}")));
                            return;
                        }
                    }
                    done += 1;
                    let _ = tx.send(DumpMsg::Progress { done, total });
                }
            }
            let [(_, user), (_, factory)] = banks;
            let _ = tx.send(DumpMsg::Done { user, factory });
        });
        Self { rx }
    }

    /// Odbiera oczekujące zdarzenia (nieblokująco). Pusty wektor = brak nowych.
    pub fn poll(&self) -> Vec<DumpMsg> {
        let mut out = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            out.push(m);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_mg101_with_two_banks() {
        let mg = registry()
            .iter()
            .find(|d| d.id == "nux.mg101")
            .expect("MG-101 w rejestrze");
        assert_eq!(mg.manufacturer, "NUX");
        assert_eq!(mg.port_needle, "MG-101");
        assert_eq!(mg.slots_per_bank, 36); // 36 User + 36 Factory = dump 72
    }

    #[test]
    fn occupied_heuristic_ignores_all_zero_blob() {
        let empty = SlotDump {
            bank: "user".into(),
            index: 0,
            blob: vec![0u8; 189],
        };
        assert!(!empty.occupied());
        let full = SlotDump {
            bank: "user".into(),
            index: 1,
            blob: {
                let mut b = vec![0u8; 189];
                b[10] = 0x42;
                b
            },
        };
        assert!(full.occupied());
    }
}

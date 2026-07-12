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

use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use mg101_device_link::{input_port_names, DeviceLink, LinkError, MidirLink, SysexAssembler};
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

/// Zdarzenie z trwałej sesji synchronizacji presetu (urządzenie → host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEvent {
    /// Na urządzeniu wybrano preset o numerze Program Change (0..).
    PresetChanged(u8),
}

/// Trwała sesja MIDI do **dwukierunkowej** synchronizacji wybranego presetu.
///
/// Protokół (empiria `findings.md`): wybór presetu to MIDI Program Change
/// `C0 <slot>` w obie strony — urządzenie wysyła go przy zmianie footswitchem,
/// host wysyła go, by przełączyć preset na urządzeniu. Sesja żyje w tle póki
/// uchwyt istnieje; wątek UI odpytuje [`PresetSync::poll`] w istniejącym Timerze,
/// a wybór z aplikacji zgłasza przez [`PresetSync::select`]. `Studio` pozostaje
/// jednowątkowe.
///
/// Działa OBOK [`Dumper`] — CoreMIDI/ALSA są multi-client, więc dwa połączenia
/// do tego samego portu współistnieją (nasłuch presetu nie koliduje ze zrzutem).
pub struct PresetSync {
    events: Receiver<SyncEvent>,
    cmd_tx: Sender<Vec<u8>>,
}

impl PresetSync {
    /// Otwiera trwałe łącze z portu `needle` i startuje wątek nasłuchu/wysyłki.
    /// Zwraca błąd, jeśli portu nie udało się otworzyć (fail-fast, rendez-vous).
    pub fn start(needle: &'static str) -> Result<Self, LinkError> {
        let (ev_tx, events) = channel::<SyncEvent>();
        let (cmd_tx, cmd_rx) = channel::<Vec<u8>>();
        let (open_tx, open_rx) = channel::<Result<(), LinkError>>();
        thread::spawn(move || {
            let mut link = match MidirLink::open(needle) {
                Ok(l) => {
                    let _ = open_tx.send(Ok(()));
                    l
                }
                Err(e) => {
                    let _ = open_tx.send(Err(e));
                    return;
                }
            };
            let mut asm = SysexAssembler::new();
            loop {
                // Wychodzące: surowe komunikaty MIDI z aplikacji (Program Change
                // przy wyborze presetu, Control Change przy edycji na żywo).
                loop {
                    match cmd_rx.try_recv() {
                        Ok(msg) => {
                            let _ = link.send(&msg);
                        }
                        Err(TryRecvError::Empty) => break,
                        // Uchwyt porzucony (PresetSync zniknął) → kończymy wątek.
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                // Przychodzące: ramkujemy strumień i wykrywamy Program Change.
                for msg in link.poll() {
                    for framed in asm.push(&msg) {
                        if framed.len() == 2 && (framed[0] & 0xF0) == 0xC0 {
                            let _ = ev_tx.send(SyncEvent::PresetChanged(framed[1]));
                        }
                    }
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        match open_rx.recv() {
            Ok(Ok(())) => Ok(Self { events, cmd_tx }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(LinkError::NotConnected),
        }
    }

    /// Odbiera oczekujące zdarzenia (nieblokująco).
    pub fn poll(&self) -> Vec<SyncEvent> {
        self.events.try_iter().collect()
    }

    /// Zgłasza wybór presetu w aplikacji → wysyła `C0 <slot>` na urządzenie.
    pub fn select(&self, slot: u16) {
        let _ = self.cmd_tx.send(vec![0xC0, (slot & 0x7F) as u8]);
    }

    /// Edycja na żywo: wysyła Control Change `B0 <cc> <value>` na urządzenie, by
    /// natychmiast zmienić brzmienie aktualnie aktywnego presetu (mapa CC katalogu).
    pub fn send_cc(&self, cc: u8, value: u8) {
        let _ = self
            .cmd_tx
            .send(vec![0xB0, cc & 0x7F, value.min(127)]);
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

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

/// Czy edytor QuickTone (NUX) działa równolegle. Ważne dla użytkownika: QT i nasza
/// aplikacja konkurują o port MIDI, a zmiany presetu wykonane w QT NIE emitują
/// Program Change, więc nasz nasłuch ich nie zobaczy (sync może się rozjechać).
/// Lekki sygnał ostrzegawczy — nie blokuje działania. macOS/Linux: `pgrep`.
#[cfg(not(target_arch = "wasm32"))]
pub fn quicktone_running() -> bool {
    std::process::Command::new("pgrep")
        .arg("-i")
        .arg("quicktone")
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
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

/// Numer MIDI CC centralnego ustawienia przypisania pedału EXP (0=off,1=WAH,
/// 2=EFX,3=AMP,4=MOD,5=DLY,6=RVB). Zmierzone monitorem MIDI.
pub const CC_EXP_TARGET: u8 = 0x4F; // 79

/// Zdarzenie z trwałej sesji synchronizacji presetu (urządzenie → host).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncEvent {
    /// Na urządzeniu wybrano preset o numerze Program Change (0..).
    PresetChanged(u8),
    /// Zmieniono centralne przypisanie pedału EXP (CC79, wartość 0..6).
    ExpTarget(u8),
    /// Zmieniono tempo DRUM (BPM) — z ramki SysEx `70 7E 02 19` (14-bit hi*128+lo).
    DrumTempo(u16),
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
    monitor: Receiver<Vec<u8>>,
    cmd_tx: Sender<Vec<u8>>,
}

impl PresetSync {
    /// Otwiera trwałe łącze z portu `needle` i startuje wątek nasłuchu/wysyłki.
    /// Zwraca błąd, jeśli portu nie udało się otworzyć (fail-fast, rendez-vous).
    pub fn start(needle: &'static str) -> Result<Self, LinkError> {
        let (ev_tx, events) = channel::<SyncEvent>();
        let (mon_tx, monitor) = channel::<Vec<u8>>();
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
                // Przychodzące: ramkujemy strumień, wykrywamy Program Change oraz
                // przekazujemy KAŻDY komunikat do monitora (poza realtime clock F8).
                for msg in link.poll() {
                    for framed in asm.push(&msg) {
                        if framed == [0xF8] {
                            continue;
                        }
                        if let Some(ev) = classify(&framed) {
                            let _ = ev_tx.send(ev);
                        }
                        let _ = mon_tx.send(framed);
                    }
                }
                thread::sleep(Duration::from_millis(15));
            }
        });
        match open_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                events,
                monitor,
                cmd_tx,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(LinkError::NotConnected),
        }
    }

    /// Odbiera oczekujące zdarzenia (nieblokująco).
    pub fn poll(&self) -> Vec<SyncEvent> {
        self.events.try_iter().collect()
    }

    /// Odbiera surowe komunikaty MIDI do monitora (nieblokująco). Wywoływać co
    /// tick, by kanał się nie zapychał — gdy monitor wyłączony, wynik odrzucić.
    pub fn poll_monitor(&self) -> Vec<Vec<u8>> {
        self.monitor.try_iter().collect()
    }

    /// Zgłasza wybór presetu w aplikacji → wysyła `C0 <slot>` na urządzenie.
    pub fn select(&self, slot: u16) {
        let _ = self.cmd_tx.send(vec![0xC0, (slot & 0x7F) as u8]);
    }

    /// Edycja na żywo: wysyła Control Change `B0 <cc> <value>` na urządzenie, by
    /// natychmiast zmienić brzmienie aktualnie aktywnego presetu (mapa CC katalogu).
    pub fn send_cc(&self, cc: u8, value: u8) {
        let _ = self.cmd_tx.send(vec![0xB0, cc & 0x7F, value.min(127)]);
    }

    /// Wysyła surowy komunikat (np. ramkę SysEx tempa DRUM) bez modyfikacji.
    /// Bajty muszą być kompletne (z `F0`…`F7` dla SysEx).
    pub fn send_raw(&self, bytes: Vec<u8>) {
        let _ = self.cmd_tx.send(bytes);
    }
}

/// Nagłówek ramek powiadomień urządzenia (device → host): `F0 43 58 70 7E 02 <param>`.
/// SUB=02 to kierunek „dane z urządzenia" (patrz `drum`: 00=żądanie, 01=zapis).
const NOTIFY_HEAD: [u8; 5] = [0x43, 0x58, 0x70, 0x7E, crate::drum::SUB_DATA];
/// Identyfikator parametru „tempo DRUM" w ramce powiadomienia.
const NOTIFY_DRUM_TEMPO: u8 = crate::drum::PARAM_DRUM_TEMPO;

/// Rozpoznaje pojedynczy zramkowany komunikat MIDI jako zdarzenie synchronizacji.
/// Wydzielone z pętli wątku, by dało się testować na prawdziwych bajtach z urządzenia.
fn classify(framed: &[u8]) -> Option<SyncEvent> {
    // Program Change (footswitch / wybór presetu).
    if framed.len() == 2 && (framed[0] & 0xF0) == 0xC0 {
        return Some(SyncEvent::PresetChanged(framed[1]));
    }
    // Control Change: centralne przypisanie pedału EXP.
    if framed.len() == 3 && (framed[0] & 0xF0) == 0xB0 && framed[1] == CC_EXP_TARGET {
        return Some(SyncEvent::ExpTarget(framed[2]));
    }
    // Tempo DRUM: `F0 43 58 70 7E 02 19 03 32 32 32 <hi> <lo> 00 F7` (15 B).
    // BPM = hi*128 + lo (dane SysEx są 7-bitowe), np. `01 10` = 144, `00 78` = 120.
    if framed.len() == 15
        && framed[0] == 0xF0
        && framed[1..6] == NOTIFY_HEAD
        && framed[6] == NOTIFY_DRUM_TEMPO
    {
        let bpm = (framed[11] as u16) * 128 + framed[12] as u16;
        return Some(SyncEvent::DrumTempo(bpm));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prawdziwe ramki z urządzenia (monitor MIDI, przejazd tempa 144→147→120).
    #[test]
    fn classify_decodes_real_drum_tempo_frames() {
        // BPM = hi*128 + lo.
        let cases: &[(&[u8], u16)] = &[
            (
                &[
                    0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x19, 0x03, 0x32, 0x32, 0x32, 0x01, 0x10,
                    0x00, 0xF7,
                ],
                144,
            ),
            (
                &[
                    0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x19, 0x03, 0x32, 0x32, 0x32, 0x01, 0x13,
                    0x00, 0xF7,
                ],
                147,
            ),
            (
                &[
                    0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x19, 0x03, 0x32, 0x32, 0x32, 0x00, 0x7F,
                    0x00, 0xF7,
                ],
                127,
            ),
            (
                &[
                    0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x19, 0x03, 0x32, 0x32, 0x32, 0x00, 0x78,
                    0x00, 0xF7,
                ],
                120,
            ),
        ];
        for (frame, expected) in cases {
            match classify(frame) {
                Some(SyncEvent::DrumTempo(bpm)) => assert_eq!(bpm, *expected, "ramka {frame:02X?}"),
                other => panic!("ramka {frame:02X?} → {other:?}, oczekiwano DrumTempo({expected})"),
            }
        }
    }

    #[test]
    fn classify_ignores_other_notification_frames() {
        // Powiadomienie o zmianie gałki (typ 14) — nie jest tempem.
        let knob = [
            0xF0, 0x43, 0x58, 0x70, 0x7E, 0x02, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0xF7,
        ];
        assert!(classify(&knob).is_none());
    }

    #[test]
    fn classify_decodes_program_change_and_exp() {
        assert!(matches!(
            classify(&[0xC0, 0x05]),
            Some(SyncEvent::PresetChanged(5))
        ));
        assert!(matches!(
            classify(&[0xB0, CC_EXP_TARGET, 0x01]),
            Some(SyncEvent::ExpTarget(1))
        ));
    }

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

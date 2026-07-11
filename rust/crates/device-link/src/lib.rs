//! Transport MIDI/SysEx dla warstwy urządzenia.
//!
//! Rdzeń tej warstwy — reasembler strumienia MIDI — jest przeniesiony z
//! narzędzia diagnostycznego `mg101-probe` (empiria z `findings.md`). Skleja
//! ramki SysEx `F0..F7` rozbite między pakiety i **poprawnie ramkuje komunikaty
//! kanałowe wg długości statusu** (naprawiony w probe błąd gubienia numeru/wartości
//! CC). Warstwa jest czysta i testowalna; sprzętowe backendy (midir na desktopie,
//! WebMIDI w webie) wejdą w epiku E2 za traitem [`DeviceLink`].

/// Kierunek ruchu na łączu z urządzeniem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Urządzenie → host.
    DevToHost,
    /// Host → urządzenie.
    HostToDev,
}

/// Abstrakcyjne łącze do urządzenia. Implementacje: midir (E2), WebMIDI (web).
/// Definicja minimalna na E0 — rozszerzana wraz z protokołem urządzenia.
pub trait DeviceLink {
    /// Wysyła surowy komunikat MIDI/SysEx do urządzenia.
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError>;
    /// Odbiera zaległe komunikaty (nieblokująco); puste, gdy brak.
    fn poll(&mut self) -> Vec<Vec<u8>>;
}

/// Błąd łącza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Brak urządzenia / rozłączone.
    NotConnected,
    /// Błąd transportu z opisem.
    Transport(String),
}

impl core::fmt::Display for LinkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LinkError::NotConnected => write!(f, "urządzenie niepodłączone"),
            LinkError::Transport(m) => write!(f, "błąd transportu: {m}"),
        }
    }
}

impl std::error::Error for LinkError {}

/// Całkowita długość komunikatu (ze statusem) dla danego bajtu statusu MIDI.
pub fn message_len(status: u8) -> usize {
    match status & 0xF0 {
        0x80 | 0x90 | 0xA0 | 0xB0 | 0xE0 => 3, // note off/on, aftertouch, CC, pitch bend
        0xC0 | 0xD0 => 2,                      // program change, channel pressure
        0xF0 => match status {
            0xF1 | 0xF3 => 2,
            0xF2 => 3,
            _ => 1, // F6, F8..FF (realtime/common jednobajtowe)
        },
        _ => 1,
    }
}

/// Parser strumienia MIDI: skleja SysEx `F0..F7` (możliwe rozbicie na pakiety)
/// oraz poprawnie ramkuje komunikaty kanałowe/systemowe wg długości statusu
/// (np. Control Change `Bn cc val` = 3 bajty). Obsługuje running status.
#[derive(Default)]
pub struct SysexAssembler {
    sysex: Vec<u8>,
    in_sysex: bool,
    pending: Vec<u8>,
    expected: usize,
    running_status: u8,
}

impl SysexAssembler {
    /// Nowy, pusty reasembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Podaje bajty jednego pakietu; zwraca kompletne komunikaty w kolejności.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for &b in bytes {
            // System realtime (F8..=FF): jednobajtowy, może wystąpić WSZĘDZIE —
            // także w środku SysEx czy komunikatu running-status. Przepuszczamy
            // obok, bez naruszania bufora SysEx ani `pending`.
            if (0xF8..=0xFF).contains(&b) {
                out.push(vec![b]);
                continue;
            }
            if self.in_sysex {
                if b == 0xF7 {
                    self.sysex.push(b);
                    out.push(std::mem::take(&mut self.sysex));
                    self.in_sysex = false;
                    continue;
                }
                if b >= 0x80 {
                    // Nowy bajt statusu w środku SysEx → ramka ucięta. Porzucamy
                    // niekompletny SysEx (ochrona przed niekończącym się buforem)
                    // i przetwarzamy `b` jako początek nowego komunikatu poniżej.
                    self.sysex.clear();
                    self.in_sysex = false;
                } else {
                    self.sysex.push(b);
                    continue;
                }
            }
            if b == 0xF0 {
                self.sysex.clear();
                self.sysex.push(b);
                self.in_sysex = true;
                self.pending.clear();
                continue;
            }
            if b >= 0x80 {
                // Bajt statusu (realtime F8..FF i SysEx F0 obsłużone wyżej; tu
                // 0x80..=0xF7). Running status dotyczy WYŁĄCZNIE komunikatów
                // kanałowych 0x80..=0xEF; System Common (0xF1..=0xF7) go KASUJE
                // (naprawa review E0/K2 — wcześniej ustawiał running_status=b,
                // więc osierocony terminator F7 rodził śmieciowe ramki).
                self.pending.clear();
                if b == 0xF7 {
                    // Osierocony ogon SysEx (bez F0) — ignoruj, skasuj kontekst.
                    self.running_status = 0;
                    continue;
                }
                if (0xF1..=0xF6).contains(&b) {
                    self.running_status = 0; // System Common — nie bierze udziału
                } else {
                    self.running_status = b; // kanałowe 0x80..=0xEF
                }
                self.expected = message_len(b);
                self.pending.push(b);
                if self.pending.len() == self.expected {
                    out.push(std::mem::take(&mut self.pending));
                }
            } else {
                // Bajt danych. Jeśli brak bieżącego statusu, użyj running status.
                if self.pending.is_empty() {
                    if self.running_status == 0 {
                        continue; // dane bez kontekstu — pomiń
                    }
                    self.pending.push(self.running_status);
                    self.expected = message_len(self.running_status);
                }
                self.pending.push(b);
                // `==` (nie `>=`): ramka zamyka się dokładnie przy oczekiwanej
                // długości (review E0/K2).
                if self.pending.len() == self.expected {
                    out.push(std::mem::take(&mut self.pending));
                }
            }
        }
        out
    }
}

/// Testowy dubler łącza: rejestruje wysłane komunikaty i generuje odpowiedzi
/// przez zadany domykający się responder (symulacja urządzenia). Używany w
/// testach protokołu (np. `pack-nux-mg101`) — nie wymaga sprzętu.
pub struct MockLink {
    /// Komunikaty wysłane przez hosta (w kolejności).
    pub sent: Vec<Vec<u8>>,
    inbox: std::collections::VecDeque<Vec<u8>>,
    #[allow(clippy::type_complexity)]
    responder: Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>> + Send>,
}

impl MockLink {
    /// Nowy dubler z responderem `wysłane → odpowiedzi urządzenia`.
    pub fn new(responder: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> Self {
        Self {
            sent: Vec::new(),
            inbox: std::collections::VecDeque::new(),
            responder: Box::new(responder),
        }
    }
}

impl DeviceLink for MockLink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        self.sent.push(bytes.to_vec());
        for msg in (self.responder)(bytes) {
            self.inbox.push_back(msg);
        }
        Ok(())
    }
    fn poll(&mut self) -> Vec<Vec<u8>> {
        self.inbox.drain(..).collect()
    }
}

/// Realny backend MIDI oparty o `midir` (macOS/Windows/Linux). Wykluczony z
/// wasm32 (tam transport dostarcza WebMIDI). Wymaga fizycznego urządzenia.
#[cfg(not(target_arch = "wasm32"))]
pub struct MidirLink {
    out: midir::MidiOutputConnection,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    // Połączenie wejściowe trzymane przy życiu (callback aktywny póki żyje).
    _input: midir::MidiInputConnection<()>,
}

/// Nazwy dostępnych portów wejściowych MIDI (do wykrywania urządzeń — W6).
/// Pusty wektor, gdy backend MIDI niedostępny.
#[cfg(not(target_arch = "wasm32"))]
pub fn input_port_names() -> Vec<String> {
    use midir::MidiInput;
    let Ok(input) = MidiInput::new("mg101-probe") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
impl MidirLink {
    /// Otwiera pierwszy port wejścia i wyjścia, którego nazwa zawiera `needle`.
    pub fn open(needle: &str) -> Result<Self, LinkError> {
        use midir::{Ignore, MidiInput, MidiOutput};

        let mut input =
            MidiInput::new("mg101-in").map_err(|e| LinkError::Transport(e.to_string()))?;
        input.ignore(Ignore::None); // NIE filtruj SysEx.
        let in_port = input
            .ports()
            .into_iter()
            .find(|p| {
                input
                    .port_name(p)
                    .map(|n| n.contains(needle))
                    .unwrap_or(false)
            })
            .ok_or(LinkError::NotConnected)?;

        let output =
            MidiOutput::new("mg101-out").map_err(|e| LinkError::Transport(e.to_string()))?;
        let out_port = output
            .ports()
            .into_iter()
            .find(|p| {
                output
                    .port_name(p)
                    .map(|n| n.contains(needle))
                    .unwrap_or(false)
            })
            .ok_or(LinkError::NotConnected)?;

        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let input_conn = input
            .connect(
                &in_port,
                "mg101-in",
                move |_stamp, message, _| {
                    let _ = tx.send(message.to_vec());
                },
                (),
            )
            .map_err(|e| LinkError::Transport(e.to_string()))?;
        let out = output
            .connect(&out_port, "mg101-out")
            .map_err(|e| LinkError::Transport(e.to_string()))?;

        Ok(Self {
            out,
            rx,
            _input: input_conn,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl DeviceLink for MidirLink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        self.out
            .send(bytes)
            .map_err(|e| LinkError::Transport(e.to_string()))
    }
    fn poll(&mut self) -> Vec<Vec<u8>> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cc_message_is_framed_with_number_and_value() {
        // Regresja: probe gubił numer/wartość CC (pokazywał samo "B0").
        let mut a = SysexAssembler::new();
        let out = a.push(&[0xB0, 44, 100]);
        assert_eq!(out, vec![vec![0xB0, 44, 100]]);
    }

    #[test]
    fn sysex_split_across_packets_is_reassembled() {
        let mut a = SysexAssembler::new();
        assert!(a.push(&[0xF0, 0x43, 0x58]).is_empty());
        let out = a.push(&[0x70, 0x0B, 0x01, 0xF7]);
        assert_eq!(out, vec![vec![0xF0, 0x43, 0x58, 0x70, 0x0B, 0x01, 0xF7]]);
    }

    #[test]
    fn running_status_reuses_previous_status() {
        let mut a = SysexAssembler::new();
        let out = a.push(&[0xB0, 44, 100, 45, 20]); // drugi CC bez powtórzenia statusu
        assert_eq!(out, vec![vec![0xB0, 44, 100], vec![0xB0, 45, 20]]);
    }

    #[test]
    fn program_change_is_two_bytes() {
        let mut a = SysexAssembler::new();
        assert_eq!(a.push(&[0xC0, 5]), vec![vec![0xC0, 5]]);
    }

    #[test]
    fn message_len_covers_status_classes() {
        assert_eq!(message_len(0xB0), 3);
        assert_eq!(message_len(0xC0), 2);
        assert_eq!(message_len(0xF2), 3);
        assert_eq!(message_len(0xF8), 1);
    }

    #[test]
    fn realtime_byte_passes_through_inside_sysex() {
        // Clock (F8) w środku SysEx: przepuszczony obok, SysEx nienaruszony.
        let mut a = SysexAssembler::new();
        let out = a.push(&[0xF0, 0x43, 0xF8, 0x58, 0x70, 0xF7]);
        assert_eq!(out, vec![vec![0xF8], vec![0xF0, 0x43, 0x58, 0x70, 0xF7]]);
    }

    #[test]
    fn realtime_does_not_break_running_status_cc() {
        // F8 między bajtami CC nie kasuje częściowego komunikatu.
        let mut a = SysexAssembler::new();
        let out = a.push(&[0xB0, 44, 0xF8, 100]);
        assert_eq!(out, vec![vec![0xF8], vec![0xB0, 44, 100]]);
    }

    #[test]
    fn status_byte_aborts_incomplete_sysex() {
        // Nowy status (nie F7/realtime) w środku SysEx przerywa i zaczyna nowy
        // komunikat — brak niekończącego się bufora.
        let mut a = SysexAssembler::new();
        let out = a.push(&[0xF0, 0x43, 0x58, 0xB0, 44, 100]);
        assert_eq!(out, vec![vec![0xB0, 44, 100]]);
        // Kolejny poprawny SysEx po przerwaniu składa się normalnie.
        let out2 = a.push(&[0xF0, 0x11, 0xF7]);
        assert_eq!(out2, vec![vec![0xF0, 0x11, 0xF7]]);
    }

    #[test]
    fn orphan_f7_is_ignored_and_does_not_poison_running_status() {
        // Regresja E0/K2: osierocony terminator F7 (ogon SysEx bez F0) NIE może
        // ustawić running_status=0xF7 ani zrodzić śmieciowych ramek [F7, x].
        let mut a = SysexAssembler::new();
        // Najpierw ustal running status kanałowy, potem wtrąć luźny F7.
        assert_eq!(a.push(&[0xB0, 44, 100]), vec![vec![0xB0, 44, 100]]);
        let out = a.push(&[0xF7, 45, 20]);
        // F7 zignorowany i skasował kontekst → dane 45/20 bez statusu = pominięte.
        assert!(
            out.is_empty(),
            "luźny F7 nie może rodzić ramek, było: {out:?}"
        );
    }

    #[test]
    fn system_common_cancels_running_status() {
        // System Common (np. Song Position F2) kasuje running status: kolejne
        // bajty danych bez własnego statusu są pomijane, nie doklejane do CC.
        let mut a = SysexAssembler::new();
        assert_eq!(a.push(&[0xB0, 44, 100]), vec![vec![0xB0, 44, 100]]);
        // F2 = Song Position (3 bajty), potem luźne dane.
        let out = a.push(&[0xF2, 0x10, 0x20, 55, 66]);
        assert_eq!(out, vec![vec![0xF2, 0x10, 0x20]], "F2 zamyka się na 3 B");
        // 55/66 bez statusu (running skasowany przez F2) → pominięte.
        assert!(a.push(&[77]).is_empty());
    }

    #[test]
    fn mock_link_records_sent_and_replays_responses() {
        let mut link = MockLink::new(|sent| {
            // Odpowiada echem z dołączonym znacznikiem.
            vec![[sent, &[0xEE]].concat()]
        });
        link.send(&[0x01, 0x02]).unwrap();
        assert_eq!(link.sent, vec![vec![0x01, 0x02]]);
        assert_eq!(link.poll(), vec![vec![0x01, 0x02, 0xEE]]);
        assert!(link.poll().is_empty());
    }
}

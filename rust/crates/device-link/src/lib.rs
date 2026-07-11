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
            if self.in_sysex {
                self.sysex.push(b);
                if b == 0xF7 {
                    out.push(std::mem::take(&mut self.sysex));
                    self.in_sysex = false;
                }
                continue;
            }
            if b == 0xF0 {
                self.sysex.clear();
                self.sysex.push(b);
                self.in_sysex = true;
                self.pending.clear();
                continue;
            }
            if b >= 0x80 {
                // Nowy bajt statusu.
                self.running_status = b;
                self.expected = message_len(b);
                self.pending.clear();
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
                if self.pending.len() >= self.expected {
                    out.push(std::mem::take(&mut self.pending));
                }
            }
        }
        out
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
}

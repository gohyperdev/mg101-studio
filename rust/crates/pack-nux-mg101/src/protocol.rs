//! Protokół SysEx MG-101 (odczyt/zapis slotów na żywo).
//!
//! Ramka: `F0 43 58 70 <TYPE> <SUB> <IDX> [payload] F7` (empiria `findings.md`).
//! `43 58` = prefiks producenta NUX/Cherub, `70` = model. `SUB`: `00`=żądanie,
//! `01`=zapis, `02`=dane. Rekord slotu (`TYPE 0B`) to **189 B na łączu**; capture
//! 04b pokazuje payload zapisu dokładnie 189 B o wartościach 0x00–0x64 (naturalnie
//! 7-bitowe) — więc to jest forma wire, a warstwa operuje na niej wprost.
//!
//! Zakres i rzeczy hardware-gated (do potwierdzenia na fizycznym MG-101):
//! - mapowanie `bank→indeks urządzenia` (`user`=0.., `factory`=36..) — PROWIZORYCZNE;
//! - **transkodowanie rekord urządzenia (189 B) ↔ plik `.mg101patch` (8402 B)** —
//!   ZAPARKOWANE (to inny kodek niż wire; plik ma osadzony IR — `findings.md`);
//! - pełny preset to para ramek `09` (54 B) + `0B` (189 B); tu obsługiwana jest
//!   część `0B` (parametry), część `09` poza zakresem E2;
//! - `write_slot` jest fire-and-forget; sprzęt ACK-uje zapis (capture 04b) —
//!   oczekiwanie na ACK do dodania przy integracji sprzętowej (BACKLOG).

use mg101_device_link::{DeviceLink, SysexAssembler};
use mg101_device_pack_api::{DeviceProtocol, ProtocolError, SlotAddr};

const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
/// Nagłówek producenta+model: `43 58 70`.
const HEADER: [u8; 3] = [0x43, 0x58, 0x70];
/// Typ rekordu slotu (189 B).
const TYPE_SLOT: u8 = 0x0B;
const SUB_REQUEST: u8 = 0x00;
const SUB_WRITE: u8 = 0x01;
const SUB_DATA: u8 = 0x02;
/// Długość surowego rekordu slotu (bajty parametrów) — wartość semantyczna;
/// na łączu payload jest 7-bitowo zakodowany (dłuższy). Transkodowanie parkowane.
pub const SLOT_RECORD_LEN: usize = 189;

/// Czy sekwencja jest poprawnym payloadem SysEx (wszystkie bajty 7-bitowe).
fn is_7bit(payload: &[u8]) -> bool {
    payload.iter().all(|&b| b < 0x80)
}

/// Bazowe indeksy banków w przestrzeni adresowej urządzenia (prowizoryczne).
const USER_BASE: u16 = 0;
const FACTORY_BASE: u16 = 36;

/// Limit oczekiwania na ramkę danych ze sprzętu.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1000);
/// Odstęp między nieblokującymi odpytaniami łącza.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

/// Protokół MG-101.
pub struct Mg101Protocol;

impl Mg101Protocol {
    /// Buduje ramkę SysEx `F0 43 58 70 TYPE SUB IDX [payload] F7`.
    fn frame(sub: u8, idx: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = Vec::with_capacity(7 + payload.len() + 1);
        f.push(SYSEX_START);
        f.extend_from_slice(&HEADER);
        f.push(TYPE_SLOT);
        f.push(sub);
        f.push(idx);
        f.extend_from_slice(payload);
        f.push(SYSEX_END);
        f
    }

    /// Mapuje adres slotu na indeks urządzenia (prowizoryczne).
    fn device_index(addr: &SlotAddr) -> Result<u8, ProtocolError> {
        let base = match addr.bank.as_str() {
            "user" => USER_BASE,
            "factory" => FACTORY_BASE,
            _ => return Err(ProtocolError::BadAddr(addr.clone())),
        };
        // Liczenie w u32 chroni przed przepełnieniem u16 przy dużym indeksie.
        u8::try_from(u32::from(base) + u32::from(addr.index))
            .map_err(|_| ProtocolError::BadAddr(addr.clone()))
    }

    /// Wyodrębnia payload z ramki danych `0B 02 <idx> <payload> F7` dla `idx`.
    fn parse_data(frame: &[u8], idx: u8) -> Option<Vec<u8>> {
        // F0 43 58 70 0B 02 IDX ... F7  → min. 8 bajtów.
        if frame.len() < 8
            || frame[0] != SYSEX_START
            || frame[1..4] != HEADER
            || frame[4] != TYPE_SLOT
            || frame[5] != SUB_DATA
            || frame[6] != idx
            || *frame.last().unwrap() != SYSEX_END
        {
            return None;
        }
        Some(frame[7..frame.len() - 1].to_vec())
    }
}

impl DeviceProtocol for Mg101Protocol {
    fn read_slot(
        &self,
        link: &mut dyn DeviceLink,
        addr: &SlotAddr,
    ) -> Result<Vec<u8>, ProtocolError> {
        let idx = Self::device_index(addr)?;
        link.send(&Self::frame(SUB_REQUEST, idx, &[]))
            .map_err(|e| ProtocolError::BadResponse(e.to_string()))?;

        // Zbieramy odpowiedzi (mogą być rozbite na pakiety, mogą być poprzedzone
        // szumem) do momentu znalezienia ramki danych naszego indeksu albo
        // upływu deadline'u. `MidirLink::poll` jest nieblokujące — odpytujemy
        // z krótkim odstępem, aż sprzęt odpowie.
        let mut asm = SysexAssembler::new();
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        while std::time::Instant::now() < deadline {
            for p in link.poll() {
                for frame in asm.push(&p) {
                    if let Some(payload) = Self::parse_data(&frame, idx) {
                        return Ok(payload);
                    }
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Err(ProtocolError::BadResponse(format!(
            "brak ramki danych dla slotu idx={idx} w limicie czasu"
        )))
    }

    fn write_slot(
        &self,
        link: &mut dyn DeviceLink,
        addr: &SlotAddr,
        blob: &[u8],
    ) -> Result<(), ProtocolError> {
        // Payload wire musi być 7-bitowy (inwariant SysEx). Kodowanie surowego
        // rekordu do 7-bit robi transkoder wire↔rekord (zaparkowany do E2/sprzęt).
        if blob.is_empty() || !is_7bit(blob) {
            return Err(ProtocolError::BadResponse(
                "payload zapisu musi być niepusty i 7-bitowy (0x00..=0x7F)".into(),
            ));
        }
        let idx = Self::device_index(addr)?;
        link.send(&Self::frame(SUB_WRITE, idx, blob))
            .map_err(|e| ProtocolError::BadResponse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mg101_device_link::MockLink;
    use mg101_device_pack_api::{BankInfo, ProtocolError};

    /// Fałszywe MG-101: na żądanie odczytu (`SUB 00`) odsyła ramkę danych z
    /// deterministycznym payloadem 7-bitowym `payload[i] = (idx + i) & 0x7F`.
    fn fake_device() -> MockLink {
        MockLink::new(|sent| {
            // sent: F0 43 58 70 0B 00 IDX F7
            if sent.len() == 8 && sent[4] == TYPE_SLOT && sent[5] == SUB_REQUEST {
                let idx = sent[6];
                let payload: Vec<u8> = (0..SLOT_RECORD_LEN)
                    .map(|i| ((idx as usize + i) & 0x7F) as u8)
                    .collect();
                vec![Mg101Protocol::frame(SUB_DATA, idx, &payload)]
            } else {
                vec![]
            }
        })
    }

    #[test]
    fn read_slot_sends_request_and_parses_data() {
        let mut link = fake_device();
        let addr = SlotAddr {
            bank: "user".into(),
            index: 3,
        };
        let payload = Mg101Protocol.read_slot(&mut link, &addr).unwrap();
        assert_eq!(payload.len(), SLOT_RECORD_LEN);
        assert_eq!(payload[0], 3);
        assert_eq!(payload[1], 4);
        // Wysłano dokładnie ramkę żądania.
        assert_eq!(link.sent, vec![Mg101Protocol::frame(SUB_REQUEST, 3, &[])]);
    }

    #[test]
    fn factory_index_offset_applied() {
        let mut link = fake_device();
        let addr = SlotAddr {
            bank: "factory".into(),
            index: 0,
        };
        let payload = Mg101Protocol.read_slot(&mut link, &addr).unwrap();
        assert_eq!(payload[0], FACTORY_BASE as u8); // idx=36
    }

    #[test]
    fn read_bank_dumps_all_slots_via_default_impl() {
        let mut link = fake_device();
        let info = BankInfo {
            id: "user".into(),
            label: "User".into(),
            slots: 36,
            index_base: 0,
            writable: true,
            reorderable: true,
            erasable: false,
        };
        let bank: Vec<Vec<u8>> = Mg101Protocol
            .read_bank(&mut link, &"user".to_string(), &info)
            .unwrap();
        assert_eq!(bank.len(), 36);
        assert!(bank.iter().all(|s| s.len() == SLOT_RECORD_LEN));
        assert_eq!(bank[5][0], 5); // slot 5 → idx 5
    }

    #[test]
    fn frame_bytes_match_findings_grammar() {
        // Golden: ramka żądania odczytu slotu idx=3 wg findings.md.
        assert_eq!(
            Mg101Protocol::frame(SUB_REQUEST, 3, &[]),
            vec![0xF0, 0x43, 0x58, 0x70, 0x0B, 0x00, 0x03, 0xF7]
        );
        // Golden: ramka zapisu z 3-bajtowym payloadem.
        assert_eq!(
            Mg101Protocol::frame(SUB_WRITE, 5, &[0x10, 0x20, 0x30]),
            vec![0xF0, 0x43, 0x58, 0x70, 0x0B, 0x01, 0x05, 0x10, 0x20, 0x30, 0xF7]
        );
    }

    #[test]
    fn read_slot_reassembles_split_frame_and_ignores_noise() {
        // Sprzęt: najpierw ramka-szum (inny TYPE 0x14), potem ramka danych
        // rozbita na dwa pakiety. read_slot ma ją złożyć i pominąć szum.
        let mut link = MockLink::new(|sent| {
            if sent.len() == 8 && sent[5] == SUB_REQUEST {
                let idx = sent[6];
                let payload: Vec<u8> = (0..8).map(|i| (i as u8) & 0x7F).collect();
                let data = Mg101Protocol::frame(SUB_DATA, idx, &payload);
                let (a, b) = data.split_at(4);
                let noise = vec![0xF0, 0x43, 0x58, 0x70, 0x14, 0x02, 0x00, 0xF7];
                vec![noise, a.to_vec(), b.to_vec()]
            } else {
                vec![]
            }
        });
        let addr = SlotAddr {
            bank: "user".into(),
            index: 7,
        };
        let payload = Mg101Protocol.read_slot(&mut link, &addr).unwrap();
        assert_eq!(payload, (0..8).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn write_slot_frames_correctly_and_validates_7bit() {
        let mut link = MockLink::new(|_| vec![]);
        let addr = SlotAddr {
            bank: "user".into(),
            index: 2,
        };
        let blob = vec![0x7Fu8; SLOT_RECORD_LEN];
        Mg101Protocol.write_slot(&mut link, &addr, &blob).unwrap();
        let sent = &link.sent[0];
        assert_eq!(sent[0], SYSEX_START);
        assert_eq!(sent[1..4], HEADER);
        assert_eq!(sent[4], TYPE_SLOT);
        assert_eq!(sent[5], SUB_WRITE);
        assert_eq!(sent[6], 2);
        assert_eq!(*sent.last().unwrap(), SYSEX_END);
        assert_eq!(sent.len(), 7 + SLOT_RECORD_LEN + 1);

        // Payload nie-7-bitowy → błąd, bez wysyłki.
        let mut link2 = MockLink::new(|_| vec![]);
        let err = Mg101Protocol.write_slot(&mut link2, &addr, &[0x80u8; 10]);
        assert!(matches!(err, Err(ProtocolError::BadResponse(_))));
        assert!(link2.sent.is_empty());
    }

    #[test]
    fn unknown_bank_is_bad_addr() {
        let mut link = MockLink::new(|_| vec![]);
        let addr = SlotAddr {
            bank: "nope".into(),
            index: 0,
        };
        assert!(matches!(
            Mg101Protocol.read_slot(&mut link, &addr),
            Err(ProtocolError::BadAddr(_))
        ));
    }
}

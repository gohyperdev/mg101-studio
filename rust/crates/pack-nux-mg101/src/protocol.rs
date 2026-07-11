//! Protokół SysEx MG-101 (odczyt/zapis slotów na żywo).
//!
//! Ramka: `F0 43 58 70 <TYPE> <SUB> <IDX> [payload] F7` (empiria `findings.md`).
//! `43 58` = prefiks producenta NUX/Cherub, `70` = model. `SUB`: `00`=żądanie,
//! `01`=zapis, `02`=dane. Rekord slotu (`TYPE 0B`) to 189 B (same parametry,
//! bez IR) — inny niż kontener `.mg101patch` (8402 B) z E1.
//!
//! UWAGA (do potwierdzenia na sprzęcie w E2): (1) mapowanie `bank→indeks
//! urządzenia` (`user`=0.., `factory`=36..); (2) **kodowanie payloadu na strumień
//! 7-bitowy** — MIDI SysEx dopuszcza w danych tylko wartości `0x00..=0x7F`, więc
//! surowy 189-bajtowy rekord (bajty 0..255) jest na łączu 7-bitowo zakodowany
//! (nibble/high-bit — do zdekodowania na sprzęcie). Ta warstwa operuje na
//! payloadzie WIRE (7-bit); transkodowanie wire↔rekord jest ZAPARKOWANE.

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
        u8::try_from(base + addr.index).map_err(|_| ProtocolError::BadAddr(addr.clone()))
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

        // Zbieramy odpowiedzi (mogą być rozbite na pakiety) i szukamy ramki
        // danych dla naszego indeksu. Backend sprzętowy dostarcza bajty między
        // kolejnymi `poll` (timing po stronie backendu); tu limit iteracji.
        let mut asm = SysexAssembler::new();
        for _ in 0..1024 {
            let packets = link.poll();
            if packets.is_empty() {
                break;
            }
            for p in packets {
                for frame in asm.push(&p) {
                    if let Some(payload) = Self::parse_data(&frame, idx) {
                        return Ok(payload);
                    }
                }
            }
        }
        Err(ProtocolError::BadResponse(format!(
            "brak ramki danych dla slotu idx={idx}"
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
    fn write_slot_frames_correctly_and_validates_len() {
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

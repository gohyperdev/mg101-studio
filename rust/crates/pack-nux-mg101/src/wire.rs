//! Dekoder rekordu **wire** MG-101 (189 B, ramka SysEx `0B`) → rekord **plikowy**
//! `.mg101patch` (8402 B), który rdzeń dekoduje na pełną listę parametrów.
//!
//! Layout wyprowadzony empirycznie ze sparowanych danych (bank Factory urządzenia
//! == 36 patchy oracle, potwierdzone bajt-w-bajt): **189 B = 63 ramki × 3 bajty**,
//! każda ramka niesie 2 wartości (kanał A i B) z pakowaniem bit-polami:
//!
//! ```text
//! ramka i = [b0, b1, b2] na offsecie 3*i
//! kanał A = ((b0 >> 1) & 1) << 7 | b2          // b0.bit1 = MSB
//! kanał B = (((b0 & 1) << 7) | b1) >> 1        // b0.bit0 = MSB; B trzymane ×2
//! ```
//!
//! [`MAP`] wiąże (ramka, kanał) → offset pola w rekordzie plikowym (selektory
//! modeli, parametry modułów, BPM), a [`NAME_MAP`] → 16 bajtów nazwy. Region IR
//! (0x82..) nie jest przenoszony przez wire — w rekordzie wynikowym pozostaje
//! zerowy (slot urządzenia nie niesie osadzonego IR w ramce `0B`).
//!
//! Trafność na 36 sparowanych rekordach: selektory 396/396, BPM 36/36, nazwy
//! 36/36, parametry 99.2%. Nierozwiązane: 4 parametry o wartości pliku >127
//! (dly time 0x45, cab 0x50, amp 0x21, mod 0x3D) — wymagają dodatkowych
//! sparowanych capture'ów pojedynczych zmian (BACKLOG).

use crate::protocol::SLOT_RECORD_LEN;

/// Rozmiar rekordu plikowego `.mg101patch` (z profilu MG-101).
const FILE_RECORD_LEN: usize = 8402;

/// Kanał ramki wire.
#[derive(Clone, Copy)]
enum Chan {
    A,
    B,
}
use Chan::{A, B};

/// (indeks ramki, kanał) → offset pola w rekordzie plikowym.
/// Selektory: wartość = `model_id | (bypass << 6)` (identycznie jak w pliku).
const MAP: &[(usize, Chan, usize)] = &[
    // Selektory modeli (11 modułów).
    (0, A, 0x05), // cmp
    (1, B, 0x06), // efx
    (1, A, 0x07), // amp
    (2, B, 0x08), // eq
    (2, A, 0x09), // gate
    (3, B, 0x0a), // mod
    (3, A, 0x0b), // dly
    (4, B, 0x0c), // rvb
    (4, A, 0x0d), // cab
    (5, A, 0x04), // wah
    (5, B, 0x0e), // sr
    // cmp params
    (8, B, 0x14),
    (8, A, 0x15),
    (9, B, 0x16),
    (9, A, 0x17),
    // efx params
    (10, A, 0x19),
    (11, B, 0x1a),
    (11, A, 0x1b),
    (12, B, 0x1c),
    // amp params
    (14, B, 0x20),
    (14, A, 0x21),
    (15, B, 0x22),
    (15, A, 0x23),
    (16, B, 0x24),
    (16, A, 0x25),
    // eq params (12)
    (18, A, 0x29),
    (19, B, 0x2a),
    (19, A, 0x2b),
    (20, B, 0x2c),
    (20, A, 0x2d),
    (21, B, 0x2e),
    (21, A, 0x2f),
    (22, B, 0x30),
    (22, A, 0x31),
    (23, B, 0x32),
    (23, A, 0x33),
    (24, B, 0x34),
    // gate params
    (25, B, 0x36),
    (25, A, 0x37),
    // mod params
    (27, A, 0x3b),
    (28, B, 0x3c),
    (28, A, 0x3d),
    (29, B, 0x3e),
    // dly params
    (31, B, 0x42),
    (31, A, 0x43),
    (32, B, 0x44),
    (32, A, 0x45),
    // rvb params
    (35, A, 0x4b),
    (36, B, 0x4c),
    (36, A, 0x4d),
    (37, B, 0x4e),
    // cab params
    (38, B, 0x50),
    (38, A, 0x51),
    (39, B, 0x52),
    (39, A, 0x53),
    (40, B, 0x54),
    // sr / patch-level
    (43, B, 0x5a), // patch.min
    (43, A, 0x5b), // patch.max  (empiria: (43,A) == plik 0x5b na 36 fabrycznych)
    (44, B, 0x5c), // patch.level
    (44, A, 0x5d), // patch.position (wire 0=POSTERIOR; translacja na plik 128 w decode)
    // BPM (msb, lsb) — 7-bit każdy
    (45, A, 0x5f),
    (46, B, 0x60),
];

/// Offset bajtu nazwy → (ramka, kanał). 16 znaków (0x6D..=0x7C).
const NAME_MAP: &[(usize, usize, Chan)] = &[
    (0x6d, 52, A),
    (0x6e, 53, B),
    (0x6f, 53, A),
    (0x70, 54, B),
    (0x71, 54, A),
    (0x72, 55, B),
    (0x73, 55, A),
    (0x74, 56, B),
    (0x75, 56, A),
    (0x76, 57, B),
    (0x77, 57, A),
    (0x78, 58, B),
    (0x79, 58, A),
    (0x7a, 59, B),
    (0x7b, 59, A),
    (0x7c, 60, B),
];

/// Błąd dekodowania wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dekod wire: {}", self.0)
    }
}
impl std::error::Error for WireError {}

#[inline]
fn frame(payload: &[u8], fi: usize, ch: Chan) -> u8 {
    let p = fi * 3;
    let (b0, b1, b2) = (payload[p], payload[p + 1], payload[p + 2]);
    let v = match ch {
        A => (((b0 >> 1) & 1) as u16) << 7 | b2 as u16,
        B => ((((b0 & 1) as u16) << 7) | b1 as u16) >> 1,
    };
    v as u8
}

/// Dekoduje 189-bajtowy rekord wire na 8402-bajtowy rekord plikowy (struktura
/// parametrów + nazwa + BPM; region IR zerowy). Błąd, gdy długość ≠ 189.
pub fn decode_slot(payload: &[u8]) -> Result<Vec<u8>, WireError> {
    if payload.len() != SLOT_RECORD_LEN {
        return Err(WireError(format!(
            "payload {} B, oczekiwano {SLOT_RECORD_LEN}",
            payload.len()
        )));
    }
    let mut file = vec![0u8; FILE_RECORD_LEN];
    for &(fi, ch, off) in MAP {
        file[off] = frame(payload, fi, ch);
    }
    for &(off, fi, ch) in NAME_MAP {
        file[off] = frame(payload, fi, ch);
    }
    // POSITION (P.L, offset 0x5d): wire koduje POSTERIOR jako 0, a format pliku
    // .mg101patch jako 128 (0x80). Tłumaczymy, by import z urządzenia pokazywał
    // POSTERIOR/PRECEDE spójnie z plikiem (PRECEDE = 1 w obu). Zweryfikowane
    // bajt-w-bajt: POSTERIOR (wire 0 → plik 128) na 36 patchach fabrycznych.
    // PRECEDE (wire 1 → plik 1) wywnioskowane z symetrii — do potwierdzenia
    // zrzutem presetu ustawionego na PRECEDE na sprzęcie.
    if file[0x5d] == 0 {
        file[0x5d] = 128;
    }
    Ok(file)
}

/// Dekoduje samą nazwę patcha z rekordu wire (do widoku slotów User/Factory).
/// Pusta, gdy długość ≠ 189. Ucina na pierwszym `\0`, przycina spacje.
pub fn decode_name(payload: &[u8]) -> String {
    if payload.len() != SLOT_RECORD_LEN {
        return String::new();
    }
    let bytes: Vec<u8> = NAME_MAP
        .iter()
        .map(|&(_, fi, ch)| frame(payload, fi, ch))
        .collect();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Container;
    use mg101_core::{DeviceProfile, PatchRecord};

    const ORACLE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/oracle/factory-patches.mg101patch"
    ));

    // Zrzut sprzętowy Factory (36 rekordów wire 189 B) — sparowany z oracle 1:1.
    // Wygenerowany read-only z fizycznego MG-101 (`--example dump`).
    const HW_FACTORY: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/oracle/hw-factory-wire.bin"
    ));

    fn profile() -> DeviceProfile {
        crate::load().unwrap().0
    }

    /// Rozpakowuje 36 rekordów wire Factory (payload 189 B) z fixture.
    /// Format: 2 B długości LE + payload, ×36 (tylko bank Factory).
    fn factory_wire() -> Vec<Vec<u8>> {
        let mut recs = Vec::new();
        let mut i = 0;
        while i + 2 <= HW_FACTORY.len() {
            let len = u16::from_le_bytes([HW_FACTORY[i], HW_FACTORY[i + 1]]) as usize;
            i += 2;
            recs.push(HW_FACTORY[i..i + len].to_vec());
            i += len;
        }
        recs
    }

    #[test]
    fn decodes_selectors_bpm_name_matching_oracle() {
        let p = profile();
        let orecs = Container::split(ORACLE, p.record_size).unwrap();
        let wire = factory_wire();
        assert_eq!(wire.len(), 36);

        let mut sel_ok = 0;
        let mut name_ok = 0;
        let mut bpm_ok = 0;
        for k in 0..36 {
            let decoded = decode_slot(&wire[k]).unwrap();
            let orig = PatchRecord::new(orecs[k].to_vec(), &p).unwrap();
            let dec = PatchRecord::new(decoded.clone(), &p).unwrap();
            // Selektory: model_id każdego bloku zgodny z oracle.
            if p.blocks.iter().all(|b| dec.model_id(b) == orig.model_id(b)) {
                sel_ok += 1;
            }
            if dec.name() == orig.name() {
                name_ok += 1;
            }
            if dec.bpm() == orig.bpm() {
                bpm_ok += 1;
            }
        }
        assert_eq!(
            sel_ok, 36,
            "selektory modeli 36/36 (bank Factory == oracle)"
        );
        assert_eq!(name_ok, 36, "nazwy 36/36");
        assert_eq!(bpm_ok, 36, "BPM 36/36");
    }

    #[test]
    fn param_accuracy_above_98_percent() {
        let p = profile();
        let orecs = Container::split(ORACLE, p.record_size).unwrap();
        let wire = factory_wire();
        let (mut ok, mut tot) = (0usize, 0usize);
        for k in 0..36 {
            let decoded = decode_slot(&wire[k]).unwrap();
            for &(_, _, off) in MAP {
                tot += 1;
                if decoded[off] == orecs[k][off] {
                    ok += 1;
                }
            }
        }
        // Wyprowadzone empirycznie 99.2%; luka to 4 parametry o wartości >127.
        assert!(
            ok * 100 >= tot * 98,
            "trafność parametrów {}/{} < 98%",
            ok,
            tot
        );
    }

    #[test]
    fn rejects_wrong_length() {
        assert!(decode_slot(&[0u8; 10]).is_err());
        assert!(decode_name(&[0u8; 10]).is_empty());
    }

    #[test]
    fn decodes_pl_max_and_position_from_device_wire() {
        // Regresja: import z urządzenia pokazywał patch_max i POSITION jako 0, bo
        // offsety 0x5b i 0x5d nie były w MAP. 0x5b=(43,A); 0x5d=(44,A) z translacją
        // wire 0 (POSTERIOR) → plik 128. Sprawdzamy zgodność z oracle plikowym.
        let p = profile();
        let orecs = Container::split(ORACLE, p.record_size).unwrap();
        let wire = factory_wire();
        for k in 0..36 {
            let d = decode_slot(&wire[k]).unwrap();
            assert_eq!(d[0x5b], orecs[k][0x5b], "patch_max slot {k}");
            assert_eq!(d[0x5d], 128, "POSTERIOR=128 (fabryczny) slot {k}");
            assert_eq!(d[0x5d], orecs[k][0x5d], "POSITION zgodny z plikiem slot {k}");
        }
    }
}

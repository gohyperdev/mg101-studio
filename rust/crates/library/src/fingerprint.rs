//! Fingerprint patcha (HLD §4).
//!
//! Dwa hashe: `exact_hash` (cały blob — bit-identyczność) oraz `content_hash`
//! (blob z **wymaskowanymi** regionami nieistotnymi brzmieniowo — nazwa i pola
//! deklarowane w profilu). Dzięki maskowaniu „ten sam dźwięk pod inną nazwą" daje
//! ten sam `content_hash` → dopasowanie brzmieniowe po połączeniu.
//!
//! Maskowanie jest **device-agnostyczne**: regiony to dane (`MaskRange`) z profilu
//! Device Packa; biblioteka nie wie, co oznaczają — tylko zeruje bajty przed hashem.
//!
//! **KONTRAKT PRZESTRZENI HASHY (krytyczne dla silnika sync i transferu E5):**
//! wszystkie hashe fingerprint MUSZĄ być liczone nad **reprezentacją rekordu
//! urządzenia** — tym, co faktycznie trafia do/ze slotu — a nie nad kontenerem
//! pliku `.mg101patch`. Inaczej patch zaimportowany z pliku (kontener 8402 B) i
//! ten sam patch odczytany ze slotu (rekord 189 B) dałyby różne hashe i silnik
//! (`sync.rs`) wiecznie raportowałby `DeviceModified`. Konsekwencja: przy imporcie
//! pliku Device Pack najpierw wyłuskuje rekord urządzenia, i to jego bajty stają
//! się `blob` w [`crate::LibraryPatch`]; `hash_at_transfer` w linku liczony jest
//! nad tymi samymi bajtami. (HLD §4 zaktualizowane.)

use crate::Hash;
use sha2::{Digest, Sha256};

/// Region bajtów do wymaskowania (pominięcia) w fingerprincie brzmieniowym.
/// Pochodzi z profilu urządzenia (np. region nazwy patcha).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskRange {
    pub start: usize,
    pub len: usize,
}

fn sha256_hex(bytes: &[u8]) -> Hash {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Hash całego blobu (bit-identyczność).
pub fn exact_hash(blob: &[u8]) -> Hash {
    sha256_hex(blob)
}

/// Fingerprint brzmieniowy: hash blobu z wyzerowanymi regionami z `masks`.
/// Regiony wykraczające poza `blob` są przycinane (bezpieczne dla różnych
/// rozmiarów rekordów). Pusta lista masek → `content_hash == exact_hash`.
pub fn content_hash(blob: &[u8], masks: &[MaskRange]) -> Hash {
    if masks.is_empty() {
        return sha256_hex(blob);
    }
    let mut normalized = blob.to_vec();
    for m in masks {
        let start = m.start.min(normalized.len());
        let end = m.start.saturating_add(m.len).min(normalized.len());
        if start < end {
            for b in &mut normalized[start..end] {
                *b = 0;
            }
        }
    }
    sha256_hex(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_hash_is_stable_and_sensitive() {
        assert_eq!(exact_hash(b""), content_hash(b"", &[]));
        assert_ne!(exact_hash(b"abc"), exact_hash(b"abd"));
    }

    #[test]
    fn masked_name_region_yields_same_content_hash() {
        // Dwa patche identyczne poza regionem nazwy [1..4) → ten sam content_hash.
        let a = [10, b'A', b'B', b'C', 20];
        let b = [10, b'X', b'Y', b'Z', 20];
        let mask = [MaskRange { start: 1, len: 3 }];
        assert_eq!(content_hash(&a, &mask), content_hash(&b, &mask));
        // Ale exact_hash je rozróżnia (nazwa jest częścią blobu).
        assert_ne!(exact_hash(&a), exact_hash(&b));
        // Zmiana poza maską (bajt brzmieniowy) różnicuje content_hash.
        let c = [11, b'A', b'B', b'C', 20];
        assert_ne!(content_hash(&a, &mask), content_hash(&c, &mask));
    }

    #[test]
    fn mask_out_of_bounds_is_clamped() {
        let blob = [1, 2, 3];
        // Region wykraczający poza blob nie panikuje; maskuje część w zakresie.
        let h = content_hash(&blob, &[MaskRange { start: 2, len: 100 }]);
        assert_eq!(h, content_hash(&[1, 2, 0], &[]));
        // Region całkowicie poza zakresem = brak zmian.
        assert_eq!(
            content_hash(&blob, &[MaskRange { start: 50, len: 4 }]),
            exact_hash(&blob)
        );
    }
}

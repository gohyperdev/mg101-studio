//! Device Pack NUX MG-101.
//!
//! Profil i katalog są DANYMI osadzonymi w pakiecie (te same pliki co v1 Swift).
//! Codec kontenera `.mg101patch` operuje na modelu kanonicznym z `mg101-core`.
//! Protokół SysEx (odczyt/zapis na żywo) dochodzi w E2.

use mg101_core::{DeviceProfile, EffectCatalog, ProfileError};

/// Profil urządzenia (dane) — osadzony z pakietu.
pub const PROFILE_JSON: &str = include_str!("../resources/device-profile.json");
/// Katalog efektów (dane) — osadzony z pakietu.
pub const CATALOG_JSON: &str = include_str!("../resources/effects-catalog.json");

/// Rola crate'u (znacznik zgodności).
pub const CRATE_ROLE: &str = "device-pack-nux-mg101";

/// Ładuje i waliduje profil + katalog MG-101 z osadzonych danych.
pub fn load() -> Result<(DeviceProfile, EffectCatalog), ProfileError> {
    let profile = DeviceProfile::from_json(PROFILE_JSON)?;
    let catalog = EffectCatalog::from_json(CATALOG_JSON)?;
    profile.validate(&catalog)?;
    Ok((profile, catalog))
}

/// Kontener `.mg101patch`: pojedynczy rekord (`record_size`) albo zestaw
/// urządzenia (`record_size × slots`). Dzieli/łączy bez transformacji bajtów.
pub struct Container;

impl Container {
    /// Liczba slotów w pełnym zestawie urządzenia (User bank).
    pub const DEVICE_SET_COUNT: usize = 36;

    /// Dzieli surowe dane na rekordy `record_size`. Błąd, gdy rozmiar nie jest
    /// dodatnią wielokrotnością `record_size`.
    pub fn split(data: &[u8], record_size: usize) -> Result<Vec<&[u8]>, String> {
        if record_size == 0 {
            return Err("record_size = 0".into());
        }
        if data.is_empty() || !data.len().is_multiple_of(record_size) {
            return Err(format!(
                "rozmiar {} nie jest wielokrotnością record_size {record_size}",
                data.len()
            ));
        }
        Ok(data.chunks(record_size).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mg101_core::{CanonicalPatch, PatchRecord};

    const ORACLE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../Sources/MG101Core/Resources/factory-patches.mg101patch"
    ));

    fn profile() -> DeviceProfile {
        load().expect("profil ładuje się i waliduje").0
    }

    #[test]
    fn profile_and_catalog_load_and_validate() {
        let (p, c) = load().expect("load OK");
        assert_eq!(p.id, "nux.mg101");
        assert_eq!(p.record_size, 8402);
        assert_eq!(p.blocks.len(), 11);
        // Kontrola liczności modeli wg inwentaryzacji v1.
        assert_eq!(c.models("amp").len(), 25);
        assert_eq!(c.models("cab").len(), 27);
        assert_eq!(c.models("efx").len(), 14);
    }

    #[test]
    fn oracle_splits_into_36_records() {
        let p = profile();
        let records = Container::split(ORACLE, p.record_size).expect("split OK");
        assert_eq!(records.len(), 36);
        assert_eq!(records.len(), Container::DEVICE_SET_COUNT);
    }

    #[test]
    fn container_roundtrip_is_byte_identical() {
        // Kontener: split → concat odtwarza dokładnie oryginał.
        let p = profile();
        let records = Container::split(ORACLE, p.record_size).unwrap();
        let rejoined: Vec<u8> = records.concat();
        assert_eq!(rejoined.len(), ORACLE.len());
        assert_eq!(rejoined, ORACLE);
    }

    #[test]
    fn codec_roundtrip_1to1_all_36_records() {
        // Bramka E1: decode → encode bajt-w-bajt na każdym z 36 rekordów.
        let p = profile();
        let records = Container::split(ORACLE, p.record_size).unwrap();
        let mut mismatches = 0usize;
        for (i, chunk) in records.iter().enumerate() {
            let record = PatchRecord::new(chunk.to_vec(), &p).expect("rozmiar OK");
            let canonical = CanonicalPatch::decode(&record);
            let reencoded = canonical.encode(&p);
            if &reencoded[..] != *chunk {
                mismatches += 1;
                eprintln!("rekord {i}: różnica bajtów w round-trip");
            }
        }
        assert_eq!(mismatches, 0, "round-trip 1:1 na wszystkich 36 rekordach");
    }

    #[test]
    fn decoded_fields_are_sane() {
        let p = profile();
        let records = Container::split(ORACLE, p.record_size).unwrap();
        for chunk in &records {
            let record = PatchRecord::new(chunk.to_vec(), &p).unwrap();
            // Nazwa dekoduje się jako poprawny UTF-8, BPM w zakresie 14-bit.
            let _ = record.name();
            let bpm = record.bpm();
            assert!(
                (0..=((0x7Fi64 << 7) | 0x7F)).contains(&bpm),
                "BPM mieści się w 14 bitach"
            );
            // Każdy blok ma model_id 0..=63.
            for block in &p.blocks {
                let id = record.model_id(block);
                assert!((0..=63).contains(&id));
            }
        }
    }

    // --- Wierność mutacji vs semantyka bajtowa Swift ---

    fn first_record(p: &DeviceProfile) -> PatchRecord<'_> {
        let chunk = Container::split(ORACLE, p.record_size).unwrap()[0].to_vec();
        PatchRecord::new(chunk, p).unwrap()
    }

    #[test]
    fn set_bpm_touches_only_bpm_bytes() {
        let p = profile();
        let original = first_record(&p);
        let mut edited = original.clone();
        edited.set_bpm(137).unwrap();
        assert_eq!(edited.bpm(), 137);
        for d in edited.differences(&original) {
            assert!(
                d.offset == p.bpm.msb_offset || d.offset == p.bpm.lsb_offset,
                "BPM zmienia tylko bajty msb/lsb (offset {})",
                d.offset
            );
        }
    }

    #[test]
    fn set_name_touches_only_name_range() {
        let p = profile();
        let original = first_record(&p);
        let mut edited = original.clone();
        edited.set_name("Rust Lead").unwrap();
        assert_eq!(edited.name(), "Rust Lead");
        let start = p.patch_name.offset;
        let end = start + p.patch_name.length;
        for d in edited.differences(&original) {
            assert!((start..end).contains(&d.offset));
        }
    }

    #[test]
    fn set_bypass_toggles_only_selector_bit() {
        let p = profile();
        let block = p.block("amp").unwrap();
        let original = first_record(&p);
        let mut edited = original.clone();
        let was = original.is_bypassed(block);
        edited.set_bypass(!was, block);
        assert_eq!(edited.is_bypassed(block), !was);
        // model_id zachowany, zmienia się co najwyżej bajt selektora.
        assert_eq!(edited.model_id(block), original.model_id(block));
        for d in edited.differences(&original) {
            assert_eq!(d.offset, block.selector_offset);
            assert_eq!(d.before ^ d.after, 0x40, "przełączony wyłącznie bit 0x40");
        }
    }

    #[test]
    fn set_model_clear_inactive_zeroes_nonactive_offsets() {
        let (p, c) = load().unwrap();
        let block = p.block("amp").unwrap();
        let model = c.models("amp")[0]; // pierwszy model AMP
        let original = first_record(&p);
        let mut edited = original.clone();
        let values: Vec<i64> = model.parameters.iter().map(|par| par.minimum()).collect();
        edited
            .set_model(model, block, &values, false, true)
            .unwrap();
        assert_eq!(edited.model_id(block), model.model_id);
        let active: std::collections::HashSet<usize> =
            model.parameters.iter().map(|par| par.file_offset).collect();
        // Offsety nieaktywne w modelu są wyzerowane.
        for &offset in &block.parameter_offsets {
            if !active.contains(&offset) {
                assert_eq!(edited.value_at(offset), 0);
            }
        }
        // Zmiany wyłącznie w obrębie selektora + offsetów parametrów bloku.
        let allowed: std::collections::HashSet<usize> = std::iter::once(block.selector_offset)
            .chain(block.parameter_offsets.iter().copied())
            .collect();
        for d in edited.differences(&original) {
            assert!(allowed.contains(&d.offset));
        }
    }
}

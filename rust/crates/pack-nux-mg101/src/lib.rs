//! Device Pack NUX MG-101.
//!
//! Profil i katalog są DANYMI osadzonymi w pakiecie (te same pliki co v1 Swift).
//! Codec kontenera `.mg101patch` operuje na modelu kanonicznym z `mg101-core`.
//! Protokół SysEx (odczyt/zapis na żywo) w module [`protocol`].

pub mod protocol;
pub mod wire;
pub use protocol::Mg101Protocol;

use mg101_core::{DeviceProfile, EffectCatalog, ProfileError};

/// Profil urządzenia (dane) — osadzony z pakietu.
pub const PROFILE_JSON: &str = include_str!("../resources/device-profile.json");
/// Katalog efektów (dane) — osadzony z pakietu.
pub const CATALOG_JSON: &str = include_str!("../resources/effects-catalog.json");

/// 36 patchy fabrycznych w formacie `.mg101patch` (zestaw urządzenia, 36×8402 B).
/// Służy m.in. do zasiania pustej Biblioteki przy pierwszym starcie (parytet v1).
pub const FACTORY_PATCHES: &[u8] = include_bytes!("../oracle/factory-patches.mg101patch");

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
        "/oracle/factory-patches.mg101patch"
    ));

    fn profile() -> DeviceProfile {
        load().expect("profil ładuje się i waliduje").0
    }

    #[test]
    fn catalog_yields_rich_param_metadata() {
        use mg101_core::effect_catalog::Control;
        let (_p, c) = load().expect("load OK");
        // AMP model 1 (JAZZ CLEAN): GAIN suwak z etykietą, BRIGHT przełącznik.
        let amp = c.model("amp", 1).expect("amp model 1");
        let gain = amp
            .parameters
            .iter()
            .find(|p| p.name == "gain")
            .expect("gain");
        assert_eq!(gain.label(), "GAIN");
        assert_eq!(gain.control(), Control::Slider);
        assert_eq!(gain.midi_cc, Some(24));
        let bright = amp
            .parameters
            .iter()
            .find(|p| p.name == "bright")
            .expect("bright");
        assert_eq!(
            bright.control(),
            Control::Toggle,
            "BRIGHT to przełącznik 0/1"
        );
        // Cabinet: katalog niesie jednostki (dB/Hz) i osobno oznacza parametry
        // „inferred" — obie cechy muszą przez rdzeń przechodzić.
        let cab: Vec<_> = c.models("cab").iter().flat_map(|m| &m.parameters).collect();
        assert!(
            cab.iter().any(|p| p.unit.as_deref() == Some("Hz")),
            "cab ma parametry w Hz"
        );
        assert!(
            cab.iter().any(|p| !p.is_confirmed()),
            "cab oznacza niepewne parametry"
        );
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
    fn eq_models_have_band_labels() {
        // 6-EQ i 10-EQ: parametry mają nazwy pasm (nie „parameter_N"). Semantyka
        // wywnioskowana z manuala MG-30 → oznaczona jako niepotwierdzona (marker ≈).
        let (_, c) = load().unwrap();
        let six = c.model("eq", 1).expect("6-EQ");
        assert_eq!(six.parameters.len(), 6);
        assert_eq!(six.parameters[0].label(), "100 Hz");
        assert_eq!(six.parameters[5].label(), "6.4 kHz");
        assert!(!six.parameters[0].is_confirmed(), "semantyka pasm = inferred");
        let ten = c.model("eq", 2).expect("10-EQ");
        assert_eq!(ten.parameters.len(), 12);
        assert_eq!(ten.parameters[0].label(), "31 Hz");
        assert_eq!(ten.parameters[9].label(), "16 kHz");
        assert_eq!(ten.parameters[10].label(), "Volume");
    }

    #[test]
    fn pl_position_is_enum_precede_1_posterior_128() {
        // Zmierzone empirycznie: PRECEDE=1 (export użytkownika), POSTERIOR=128
        // (0x80, domyślne we wszystkich 36 patchach fabrycznych). To 2-stanowy enum
        // o własnych wartościach — musi renderować się jako toggle, nie suwak.
        use mg101_core::effect_catalog::Control;
        let (_, c) = load().unwrap();
        let pl = c.model("sr", 1).expect("P.L");
        let pos = pl
            .parameters
            .iter()
            .find(|p| p.name == "patch_level_position")
            .expect("patch_level_position");
        assert_eq!(pos.control(), Control::Enum, "POSITION = enum (toggle)");
        assert_eq!(pos.enum_label(1), "PRECEDE");
        assert_eq!(pos.enum_label(128), "POSTERIOR");
        assert_eq!(pos.enum_next(1), 128, "PRECEDE → POSTERIOR");
        assert_eq!(pos.enum_next(128), 1, "POSTERIOR → PRECEDE (cykl)");
        // LEVEL: fizyczne dB (50 = 0 dB).
        let lvl = pl.parameters.iter().find(|p| p.name == "patch_level").unwrap();
        assert_eq!(lvl.display_value(50).as_deref(), Some("+0.0 dB"));
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

    // --- Złote wartości: pinują offsety dekodowania do prawdy naziemnej ---

    #[test]
    fn golden_decoded_values_pin_offsets() {
        let p = profile();
        let recs = Container::split(ORACLE, p.record_size).unwrap();
        let r0 = PatchRecord::new(recs[0].to_vec(), &p).unwrap();
        assert_eq!(r0.slot_index(), 0);
        assert_eq!(r0.name(), "EuroLead");
        assert_eq!(r0.bpm(), 120);
        let r17 = PatchRecord::new(recs[17].to_vec(), &p).unwrap();
        assert_eq!(r17.name(), "Mayer Clean");
        let r35 = PatchRecord::new(recs[35].to_vec(), &p).unwrap();
        assert_eq!(r35.slot_index(), 35);
        assert_eq!(r35.name(), "Uber");
        assert_eq!(r35.bpm(), 100);
    }

    // --- Bezstratność na wzorcach spoza danych fabrycznych (naprawa uwagi
    //     krytycznej z review E1): ogon nazwy po zerze, bit 0x80 selektora,
    //     górny bit bajtu BPM, bajt w regionie nieznanym profilowi. ---

    #[test]
    fn codec_roundtrip_lossless_on_adversarial_bytes() {
        let p = profile();
        let mut bytes = Container::split(ORACLE, p.record_size).unwrap()[0].to_vec();

        // Ogon nazwy: bajt po terminatorze "EuroLead"(8) w polu [109..125).
        let name_start = p.patch_name.offset;
        bytes[name_start + 8] = 0; // terminator
        bytes[name_start + 10] = 0xAA; // śmieć po zerze
                                       // Bit 0x80 selektora bloku amp (selector_offset=7).
        let amp = p.block("amp").unwrap();
        bytes[amp.selector_offset] |= 0x80;
        // Górny bit bajtu MSB BPM.
        bytes[p.bpm.msb_offset] |= 0x80;
        // Bajt w regionie nieobjętym mapą profilu (offset 100).
        bytes[100] = 0x55;

        let record = PatchRecord::new(bytes.clone(), &p).unwrap();
        let canonical = CanonicalPatch::decode(&record);
        let reencoded = canonical.encode(&p);
        assert_eq!(
            reencoded, bytes,
            "encode(decode(x)) == x dla dowolnych bajtów (bezstratność)"
        );
    }

    // --- Ścieżki błędów (parytet semantyki błędów ze Swiftem) ---

    #[test]
    fn error_paths_match_swift_semantics() {
        use mg101_core::PatchError;
        let p = profile();
        let mut r = first_record(&p);

        // BPM poza zakresem profilu (40..=300).
        assert!(matches!(r.set_bpm(9999), Err(PatchError::Bpm(9999))));
        assert!(matches!(r.set_bpm(0), Err(PatchError::Bpm(0))));

        // Nazwa dłuższa niż pole (16 B).
        let long = "X".repeat(17);
        assert!(matches!(
            r.set_name(&long),
            Err(PatchError::NameTooLong(16))
        ));

        // Nieznane pole globalne.
        assert!(matches!(
            r.set_named_field("nope", 1),
            Err(PatchError::UnknownField(_))
        ));

        // IR: zła długość i brak RIFF/WAVE.
        assert!(matches!(r.set_ir(&[0u8; 10], "x"), Err(PatchError::Ir(_))));
        let wrong_len = p.record_size - 0xA6;
        let mut not_riff = vec![0u8; wrong_len];
        not_riff[0] = b'X';
        assert!(matches!(r.set_ir(&not_riff, "x"), Err(PatchError::Ir(_))));

        // Rozmiar rekordu.
        assert!(matches!(
            PatchRecord::new(vec![0u8; 10], &p),
            Err(PatchError::InvalidSize {
                expected: 8402,
                actual: 10
            })
        ));
    }

    #[test]
    fn validate_rejects_bad_profile() {
        use mg101_core::{DeviceProfile, EffectCatalog, ProfileError};
        let bad = PROFILE_JSON.replace("\"schemaVersion\": 1", "\"schemaVersion\": 2");
        let profile = DeviceProfile::from_json(&bad).unwrap();
        let catalog = EffectCatalog::from_json(CATALOG_JSON).unwrap();
        assert!(matches!(
            profile.validate(&catalog),
            Err(ProfileError::Malformed(_))
        ));
    }
}

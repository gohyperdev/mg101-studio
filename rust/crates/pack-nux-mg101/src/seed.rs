//! Syntetyczny seed startowy Biblioteki — **generowany z profilu**, bez żadnych
//! danych producenta.
//!
//! Aplikacja NIE dostarcza fabrycznych patchy NUX (te użytkownik zaciąga sam
//! z urządzenia). Zamiast tego przy pierwszym starcie zasiewamy kilka
//! neutralnych, generycznych ustawień jako punkt wyjścia do edycji. Każdy rekord
//! ma poprawną strukturę `record_size` (8402 B): slot, nazwa, BPM i selektory
//! bloków są ustawiane przez offsety z [`DeviceProfile`], reszta to zera —
//! kodek „świętych bajtów" (ADR-0002) i tak odtwarza je bezstratnie.
//!
//! Nazwy i dobór modeli są nasze — nie odwzorowują żadnego zestawu fabrycznego.

use mg101_core::{DeviceProfile, EffectCatalog};

/// Pojedynczy generyczny preset: nazwa + BPM + model AMP + model CAB + poziom.
/// Świadomie minimalny — to punkt startowy, nie „brzmienie".
struct Preset {
    name: &'static str,
    bpm: u16,
    amp: i64,
    cab: i64,
    level: u8,
}

/// Tabela presetów. Ogólne, gatunkowo neutralne nazwy — użytkownik zmienia
/// modele i parametry na urządzeniu lub w aplikacji.
const PRESETS: &[Preset] = &[
    Preset {
        name: "Init Clean",
        bpm: 120,
        amp: 1,
        cab: 1,
        level: 80,
    },
    Preset {
        name: "Warm Crunch",
        bpm: 110,
        amp: 2,
        cab: 1,
        level: 78,
    },
    Preset {
        name: "Lead Boost",
        bpm: 130,
        amp: 3,
        cab: 1,
        level: 82,
    },
    Preset {
        name: "Ambient Clean",
        bpm: 90,
        amp: 1,
        cab: 2,
        level: 76,
    },
    Preset {
        name: "Blues Drive",
        bpm: 100,
        amp: 2,
        cab: 2,
        level: 79,
    },
    Preset {
        name: "Modern High-Gain",
        bpm: 140,
        amp: 4,
        cab: 1,
        level: 84,
    },
];

/// Buduje jeden rekord `record_size` dla presetu na pozycji `slot`.
/// Model AMP/CAB jest brany tylko, jeśli katalog go zna — inaczej blok zostaje
/// na modelu 0 (pierwszy dostępny), żeby nie wpisać nieistniejącego selektora.
fn build_record(
    profile: &DeviceProfile,
    catalog: &EffectCatalog,
    slot: u32,
    preset: &Preset,
) -> Vec<u8> {
    let mut r = vec![0u8; profile.record_size];

    // Slot (u32 LE).
    r[0..4].copy_from_slice(&slot.to_le_bytes());

    // Nazwa (ASCII, zero-padded do długości pola).
    let start = profile.patch_name.offset;
    let max = profile.patch_name.length;
    for (i, b) in preset.name.bytes().take(max).enumerate() {
        r[start + i] = b;
    }

    // BPM = msb<<7 | lsb.
    r[profile.bpm.msb_offset] = (preset.bpm >> 7) as u8;
    r[profile.bpm.lsb_offset] = (preset.bpm & 0x7F) as u8;

    // Selektor bloku = model_id w dolnych 6 bitach (bit 0x40 = bypass — zostaje 0,
    // czyli blok aktywny). Model bierzemy tylko, gdy katalog go zna.
    let set_block = |r: &mut [u8], block_id: &str, model: i64| {
        if let Some(block) = profile.blocks.iter().find(|b| b.id == block_id) {
            let valid = catalog.model(block_id, model).is_some();
            let id = if valid { model } else { 0 };
            r[block.selector_offset] = (id as u8) & 0x3F;
        }
    };
    set_block(&mut r, "amp", preset.amp);
    set_block(&mut r, "cab", preset.cab);

    // Poziom patcha (named field, jeśli obecny w profilu).
    if let Some(field) = profile.named_fields.get("patch.level") {
        r[field.offset] = preset.level.min(field.maximum as u8);
    }

    r
}

/// Generuje syntetyczny zestaw seedowy: `PRESETS.len()` rekordów po `record_size`,
/// gotowy do zapisania jako `.mg101patch` i zaimportowania do Biblioteki.
pub fn seed_patches(profile: &DeviceProfile, catalog: &EffectCatalog) -> Vec<u8> {
    let mut out = Vec::with_capacity(PRESETS.len() * profile.record_size);
    for (i, preset) in PRESETS.iter().enumerate() {
        out.extend_from_slice(&build_record(profile, catalog, i as u32, preset));
    }
    out
}

/// Liczba patchy w seedzie (dla testów/UI).
pub fn seed_count() -> usize {
    PRESETS.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mg101_core::{CanonicalPatch, PatchRecord};

    fn load() -> (DeviceProfile, EffectCatalog) {
        crate::load().expect("profil+katalog")
    }

    #[test]
    fn seed_is_whole_multiple_of_record_size() {
        let (p, c) = load();
        let data = seed_patches(&p, &c);
        assert_eq!(data.len(), PRESETS.len() * p.record_size);
        assert!(data.len().is_multiple_of(p.record_size));
    }

    #[test]
    fn every_seed_record_decodes_and_round_trips_bit_exact() {
        // Seed musi przejść tę samą ścieżkę co import: dekod → enkod bajt-w-bajt.
        let (p, c) = load();
        let data = seed_patches(&p, &c);
        for chunk in data.chunks(p.record_size) {
            let rec = PatchRecord::new(chunk.to_vec(), &p).expect("rozmiar OK");
            let canon = CanonicalPatch::decode(&rec);
            let reencoded = canon.encode(&p);
            assert_eq!(reencoded, chunk, "round-trip seedu musi być bezstratny");
        }
    }

    #[test]
    fn seed_records_carry_name_bpm_and_slot() {
        let (p, c) = load();
        let data = seed_patches(&p, &c);
        for (i, chunk) in data.chunks(p.record_size).enumerate() {
            let rec = PatchRecord::new(chunk.to_vec(), &p).unwrap();
            assert_eq!(rec.slot_index(), i as u32, "slot rosnący");
            assert_eq!(rec.name(), PRESETS[i].name, "nazwa presetu zachowana");
            assert_eq!(rec.bpm(), PRESETS[i].bpm as i64, "BPM zachowane");
        }
    }

    #[test]
    fn seed_uses_only_catalog_known_models() {
        // Regresja: nie wpisujemy selektora modelu, którego katalog nie zna.
        let (p, c) = load();
        let data = seed_patches(&p, &c);
        for chunk in data.chunks(p.record_size) {
            let rec = PatchRecord::new(chunk.to_vec(), &p).unwrap();
            let canon = CanonicalPatch::decode(&rec);
            for block in &canon.blocks {
                let id = block.model_id();
                assert!(
                    c.model(&block.id, id).is_some() || id == 0,
                    "blok {} ma model {id} spoza katalogu",
                    block.id
                );
            }
        }
    }
}

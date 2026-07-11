//! Model kanoniczny patcha + dekod/enkod (codec offline).
//!
//! Dekod rozkłada rekord na pola semantyczne (slot, nazwa, BPM, stan bloków,
//! pola globalne) i zachowuje oryginalny blob dla bajtów spoza mapy profilu
//! („bajty święte", ADR-0002). Enkod **rekonstruuje** znane regiony z pól
//! semantycznych na kopii blobu — dla danych urządzenia daje wierny round-trip
//! bajt-w-bajt, a jednocześnie dowodzi kompletności i poprawności mapy offsetów.

use crate::device_profile::DeviceProfile;
use crate::patch_record::PatchRecord;

const IR_REGION_OFFSET: usize = 0x82;

/// Stan pojedynczego bloku łańcucha sygnału.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockState {
    /// Identyfikator bloku (z profilu).
    pub id: String,
    /// ID aktywnego modelu (dolne 6 bitów selektora).
    pub model_id: i64,
    /// Bypass (bit `0x40`).
    pub bypassed: bool,
    /// Wartości bajtów pod `parameter_offsets` (w kolejności profilu).
    pub params: Vec<u8>,
}

/// Kanoniczny patch: pola semantyczne + zachowany blob źródłowy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalPatch {
    /// Indeks slotu.
    pub slot: u32,
    /// Nazwa patcha (do wyświetlania/wyszukiwania).
    pub name: String,
    /// BPM.
    pub bpm: i64,
    /// Stany bloków.
    pub blocks: Vec<BlockState>,
    /// Pola globalne (nazwa → wartość bajtu).
    pub named_fields: std::collections::BTreeMap<String, u8>,
    /// Czy obecny IR.
    pub ir_present: bool,
    /// Nazwa IR.
    pub ir_name: String,
    /// Dokładne bajty regionu IR `0x82..record_size` (dla wierności).
    ir_region: Vec<u8>,
    /// Oryginalny blob (źródło bajtów nieznanych profilowi).
    blob: Vec<u8>,
}

impl CanonicalPatch {
    /// Dekoduje rekord do modelu kanonicznego.
    pub fn decode(record: &PatchRecord) -> Self {
        let profile = record.profile();
        let data = record.data();

        let blocks = profile
            .blocks
            .iter()
            .map(|block| BlockState {
                id: block.id.clone(),
                model_id: record.model_id(block),
                bypassed: record.is_bypassed(block),
                params: block.parameter_offsets.iter().map(|&o| data[o]).collect(),
            })
            .collect();

        let named_fields = profile
            .named_fields
            .iter()
            .map(|(name, field)| (name.clone(), data[field.offset]))
            .collect();

        CanonicalPatch {
            slot: record.slot_index(),
            name: record.name(),
            bpm: record.bpm(),
            blocks,
            named_fields,
            ir_present: record.ir_present(),
            ir_name: record.ir_name(),
            ir_region: data[IR_REGION_OFFSET..profile.record_size].to_vec(),
            blob: data.to_vec(),
        }
    }

    /// Rekonstruuje bajty rekordu z pól semantycznych; bajty spoza mapy profilu
    /// pochodzą z zachowanego blobu. Dla poprawnie zdekodowanego rekordu wynik
    /// jest identyczny z oryginałem.
    pub fn encode(&self, profile: &DeviceProfile) -> Vec<u8> {
        let mut out = self.blob.clone();

        // Slot (LE, 0..4).
        out[0..4].copy_from_slice(&self.slot.to_le_bytes());

        // Nazwa (dopełnienie zerami do długości pola).
        let name_bytes = self.name.as_bytes();
        let start = profile.patch_name.offset;
        for i in 0..profile.patch_name.length {
            out[start + i] = name_bytes.get(i).copied().unwrap_or(0);
        }

        // BPM (7-bit MSB/LSB).
        out[profile.bpm.msb_offset] = ((self.bpm >> 7) & 0x7F) as u8;
        out[profile.bpm.lsb_offset] = (self.bpm & 0x7F) as u8;

        // Bloki: selektor + parametry.
        for (block, state) in profile.blocks.iter().zip(self.blocks.iter()) {
            out[block.selector_offset] =
                (state.model_id as u8 & 0x3F) | if state.bypassed { 0x40 } else { 0 };
            for (&offset, &val) in block.parameter_offsets.iter().zip(state.params.iter()) {
                out[offset] = val;
            }
        }

        // Pola globalne.
        for (name, field) in &profile.named_fields {
            if let Some(&val) = self.named_fields.get(name) {
                out[field.offset] = val;
            }
        }

        // Region IR (wierny).
        out[IR_REGION_OFFSET..profile.record_size].copy_from_slice(&self.ir_region);

        out
    }
}

//! Model kanoniczny patcha + dekod/enkod (codec offline).
//!
//! Dekod rozkłada rekord na pola semantyczne (slot, nazwa, BPM, stan bloków,
//! pola globalne) ORAZ zachowuje surowe bajty każdego regionu, którego widok
//! semantyczny jest stratny (ogon nazwy po zerze, pełny bajt selektora z bitem
//! `0x80`, oryginalne bajty BPM), a także cały region IR i pełny blob dla bajtów
//! spoza mapy profilu. Dzięki temu enkod jest **bezstratny bajt-w-bajt dla
//! dowolnego wejścia** („bajty święte", ADR-0002), a pola semantyczne służą do
//! wyświetlania/wyszukiwania/intencji edycji.

use crate::device_profile::DeviceProfile;
use crate::patch_record::PatchRecord;

const IR_REGION_OFFSET: usize = 0x82;

/// Stan pojedynczego bloku łańcucha sygnału.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockState {
    /// Identyfikator bloku (z profilu).
    pub id: String,
    /// Pełny bajt selektora (bezstratny; zawiera model_id, bypass i ewentualny
    /// bit `0x80`). Widoki semantyczne: [`BlockState::model_id`]/[`bypassed`].
    pub selector: u8,
    /// Wartości bajtów pod `parameter_offsets` (w kolejności profilu).
    pub params: Vec<u8>,
}

impl BlockState {
    /// ID aktywnego modelu (dolne 6 bitów selektora).
    pub fn model_id(&self) -> i64 {
        i64::from(self.selector & 0x3F)
    }
    /// Czy blok jest zbypassowany (bit `0x40`).
    pub fn bypassed(&self) -> bool {
        self.selector & 0x40 != 0
    }
}

/// Kanoniczny patch: pola semantyczne (widoki) + surowe bajty regionów i blob
/// dla wierności bajtowej.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalPatch {
    /// Indeks slotu (bezstratny — pełny UInt32 LE).
    pub slot: u32,
    /// Nazwa patcha (widok — ucięta na terminatorze, UTF-8 lossy).
    pub name: String,
    /// BPM (widok — `msb<<7 | lsb`).
    pub bpm: i64,
    /// Stany bloków.
    pub blocks: Vec<BlockState>,
    /// Pola globalne (nazwa → wartość bajtu; bezstratne — surowe bajty).
    pub named_fields: std::collections::BTreeMap<String, u8>,
    /// Czy obecny IR (widok).
    pub ir_present: bool,
    /// Nazwa IR (widok).
    pub ir_name: String,
    /// Surowe bajty pola nazwy (długość = `patch_name.length`) — dla wierności.
    name_raw: Vec<u8>,
    /// Surowe bajty BPM `(msb, lsb)` — dla wierności.
    bpm_raw: (u8, u8),
    /// Dokładne bajty regionu IR `0x82..record_size` — dla wierności.
    ir_region: Vec<u8>,
    /// Oryginalny blob (źródło bajtów nieznanych profilowi).
    blob: Vec<u8>,
}

impl CanonicalPatch {
    /// Dekoduje rekord do modelu kanonicznego (zachowując surowe bajty regionów).
    pub fn decode(record: &PatchRecord) -> Self {
        let profile = record.profile();
        let data = record.data();

        let blocks = profile
            .blocks
            .iter()
            .map(|block| BlockState {
                id: block.id.clone(),
                selector: data[block.selector_offset],
                params: block.parameter_offsets.iter().map(|&o| data[o]).collect(),
            })
            .collect();

        let named_fields = profile
            .named_fields
            .iter()
            .map(|(name, field)| (name.clone(), data[field.offset]))
            .collect();

        let name_start = profile.patch_name.offset;
        let name_end = name_start + profile.patch_name.length;

        CanonicalPatch {
            slot: record.slot_index(),
            name: record.name(),
            bpm: record.bpm(),
            blocks,
            named_fields,
            ir_present: record.ir_present(),
            ir_name: record.ir_name(),
            name_raw: data[name_start..name_end].to_vec(),
            bpm_raw: (data[profile.bpm.msb_offset], data[profile.bpm.lsb_offset]),
            ir_region: data[IR_REGION_OFFSET..profile.record_size].to_vec(),
            blob: data.to_vec(),
        }
    }

    /// Rekonstruuje bajty rekordu. Znane regiony pochodzą z zachowanych surowych
    /// bajtów, bajty spoza mapy profilu z blobu → wynik jest identyczny z
    /// oryginałem dla dowolnego wejścia (bezstratność).
    pub fn encode(&self, profile: &DeviceProfile) -> Vec<u8> {
        let mut out = self.blob.clone();

        // Slot (LE, 0..4) — pełny u32, bezstratny.
        out[0..4].copy_from_slice(&self.slot.to_le_bytes());

        // Nazwa — surowe bajty pola (zachowuje ogon po terminatorze).
        let start = profile.patch_name.offset;
        out[start..start + profile.patch_name.length].copy_from_slice(&self.name_raw);

        // BPM — surowe bajty.
        out[profile.bpm.msb_offset] = self.bpm_raw.0;
        out[profile.bpm.lsb_offset] = self.bpm_raw.1;

        // Bloki: pełny bajt selektora + parametry (surowe).
        for (block, state) in profile.blocks.iter().zip(self.blocks.iter()) {
            out[block.selector_offset] = state.selector;
            for (&offset, &val) in block.parameter_offsets.iter().zip(state.params.iter()) {
                out[offset] = val;
            }
        }

        // Pola globalne (surowe bajty).
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

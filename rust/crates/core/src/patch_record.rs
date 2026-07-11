//! Rekord patcha — wierny port `PatchRecord` ze Swift (MG101Core).
//!
//! Rekord to bufor bajtów o rozmiarze `record_size` z akcesorami sterowanymi
//! profilem. „Bajty święte" (ADR-0002): nieznane bajty nietknięte przy edycji.
//! Offsety regionu IR (`0x82`/`0x86`/`0xA6`) są — jak w Swift — stałymi MG-101.

use crate::device_profile::{Block, DeviceProfile};
use crate::effect_catalog::{Model, Parameter};
use crate::error::PatchError;

const IR_FLAG_OFFSET: usize = 0x82;
const IR_NAME_OFFSET: usize = 0x86;
const IR_NAME_LEN: usize = 32;
const IR_WAV_OFFSET: usize = 0xA6;

/// Różnica pojedynczego bajtu (offset + wartość przed/po).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteDifference {
    pub offset: usize,
    pub before: u8,
    pub after: u8,
}

/// Rekord patcha powiązany z profilem urządzenia.
#[derive(Debug, Clone)]
pub struct PatchRecord<'p> {
    data: Vec<u8>,
    profile: &'p DeviceProfile,
}

impl<'p> PatchRecord<'p> {
    /// Tworzy rekord, sprawdzając rozmiar względem profilu.
    pub fn new(data: Vec<u8>, profile: &'p DeviceProfile) -> Result<Self, PatchError> {
        if data.len() != profile.record_size {
            return Err(PatchError::InvalidSize {
                expected: profile.record_size,
                actual: data.len(),
            });
        }
        Ok(Self { data, profile })
    }

    /// Surowe bajty rekordu.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Profil powiązany.
    pub fn profile(&self) -> &DeviceProfile {
        self.profile
    }

    /// Indeks slotu (UInt32 LE, offset 0).
    pub fn slot_index(&self) -> u32 {
        u32::from_le_bytes([self.data[0], self.data[1], self.data[2], self.data[3]])
    }

    /// Ustawia indeks slotu (LE).
    pub fn set_slot_index(&mut self, value: u32) {
        self.data[0..4].copy_from_slice(&value.to_le_bytes());
    }

    /// Nazwa patcha (UTF-8, ucięta na pierwszym `0`).
    pub fn name(&self) -> String {
        let start = self.profile.patch_name.offset;
        let end = start + self.profile.patch_name.length;
        let slice = &self.data[start..end];
        let trimmed: Vec<u8> = slice.iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8_lossy(&trimmed).into_owned()
    }

    /// BPM = msb<<7 | lsb.
    pub fn bpm(&self) -> i64 {
        (i64::from(self.data[self.profile.bpm.msb_offset]) << 7)
            | i64::from(self.data[self.profile.bpm.lsb_offset])
    }

    /// Czy obecny jest lokalny IR (któryś z bajtów flagi `0x82..=0x85` ≠ 0).
    pub fn ir_present(&self) -> bool {
        self.data[0x82] != 0 || self.data[0x83] != 0 || self.data[0x84] != 0 || self.data[0x85] != 0
    }

    /// Nazwa IR (UTF-8, ucięta na `0`).
    pub fn ir_name(&self) -> String {
        let slice = &self.data[IR_NAME_OFFSET..IR_NAME_OFFSET + IR_NAME_LEN];
        let trimmed: Vec<u8> = slice.iter().copied().take_while(|&b| b != 0).collect();
        String::from_utf8_lossy(&trimmed).into_owned()
    }

    /// Bajt selektora bloku.
    pub fn selector(&self, block: &Block) -> u8 {
        self.data[block.selector_offset]
    }

    /// ID modelu (dolne 6 bitów selektora).
    pub fn model_id(&self, block: &Block) -> i64 {
        i64::from(self.selector(block) & 0x3F)
    }

    /// Czy blok jest zbypassowany (bit `0x40`).
    pub fn is_bypassed(&self, block: &Block) -> bool {
        self.selector(block) & 0x40 != 0
    }

    /// Wartość bajtu pod offsetem.
    pub fn value_at(&self, offset: usize) -> i64 {
        i64::from(self.data[offset])
    }

    /// Ustawia bajt (0..=255) pod offsetem, z kontrolą zakresu i granic.
    pub fn set_byte(&mut self, value: i64, offset: usize) -> Result<(), PatchError> {
        if offset >= self.data.len() {
            return Err(PatchError::Offset(offset));
        }
        if !(0..=255).contains(&value) {
            return Err(PatchError::Value(value));
        }
        self.data[offset] = value as u8;
        Ok(())
    }

    /// Ustawia/zdejmuje bypass bloku, zachowując id modelu.
    pub fn set_bypass(&mut self, bypassed: bool, block: &Block) {
        let model = self.data[block.selector_offset] & 0x3F;
        self.data[block.selector_offset] = model | if bypassed { 0x40 } else { 0 };
    }

    /// Ustawia parametr (z kontrolą zakresu z katalogu).
    pub fn set_parameter(&mut self, parameter: &Parameter, value: i64) -> Result<(), PatchError> {
        if !(parameter.minimum()..=parameter.maximum()).contains(&value) {
            return Err(PatchError::ParameterRange {
                name: parameter.name.clone(),
                minimum: parameter.minimum(),
                maximum: parameter.maximum(),
                value,
            });
        }
        self.set_byte(value, parameter.file_offset)
    }

    /// Ustawia model bloku: selektor + wszystkie parametry; opcjonalnie zeruje
    /// offsety parametrów nieaktywne w nowym modelu (wierny port `setModel`).
    pub fn set_model(
        &mut self,
        model: &Model,
        block: &Block,
        values: &[i64],
        bypassed: bool,
        clear_inactive: bool,
    ) -> Result<(), PatchError> {
        if values.len() != model.parameters.len() {
            return Err(PatchError::ParameterCount {
                expected: model.parameters.len(),
                actual: values.len(),
            });
        }
        self.data[block.selector_offset] = (model.model_id as u8) | if bypassed { 0x40 } else { 0 };
        let active: std::collections::HashSet<usize> =
            model.parameters.iter().map(|p| p.file_offset).collect();
        for (parameter, &value) in model.parameters.iter().zip(values.iter()) {
            self.set_parameter(parameter, value)?;
        }
        if clear_inactive {
            for &offset in &block.parameter_offsets {
                if !active.contains(&offset) {
                    self.data[offset] = 0;
                }
            }
        }
        Ok(())
    }

    /// Ustawia BPM (rozbicie na 7-bitowe MSB/LSB).
    pub fn set_bpm(&mut self, value: i64) -> Result<(), PatchError> {
        if !(self.profile.bpm.minimum..=self.profile.bpm.maximum).contains(&value) {
            return Err(PatchError::Bpm(value));
        }
        self.data[self.profile.bpm.msb_offset] = ((value >> 7) & 0x7F) as u8;
        self.data[self.profile.bpm.lsb_offset] = (value & 0x7F) as u8;
        Ok(())
    }

    /// Ustawia pole globalne (send/return/patch.*).
    pub fn set_named_field(&mut self, name: &str, value: i64) -> Result<(), PatchError> {
        let field = self
            .profile
            .named_fields
            .get(name)
            .ok_or_else(|| PatchError::UnknownField(name.to_string()))?;
        if !(field.minimum..=field.maximum).contains(&value) {
            return Err(PatchError::ParameterRange {
                name: name.to_string(),
                minimum: field.minimum,
                maximum: field.maximum,
                value,
            });
        }
        let offset = field.offset;
        self.set_byte(value, offset)
    }

    /// Ustawia nazwę patcha (UTF-8, dopełnienie zerami do długości pola).
    pub fn set_name(&mut self, value: &str) -> Result<(), PatchError> {
        let encoded = value.as_bytes();
        let len = self.profile.patch_name.length;
        if encoded.len() > len {
            return Err(PatchError::NameTooLong(len));
        }
        let start = self.profile.patch_name.offset;
        for index in 0..len {
            self.data[start + index] = encoded.get(index).copied().unwrap_or(0);
        }
        Ok(())
    }

    /// Osadza IR (WAV) — wierny port `setIR` z walidacją RIFF/WAVE i długości.
    pub fn set_ir(&mut self, wav: &[u8], name: &str) -> Result<(), PatchError> {
        let expected_length = self.profile.record_size - IR_WAV_OFFSET;
        if wav.len() != expected_length {
            return Err(PatchError::Ir(format!(
                "Expected {expected_length} WAV bytes, received {}.",
                wav.len()
            )));
        }
        if &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
            return Err(PatchError::Ir("IR must be a RIFF/WAVE file.".into()));
        }
        let riff_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        if riff_size != (expected_length - 8) as u32 {
            return Err(PatchError::Ir(format!("Invalid RIFF size {riff_size}.")));
        }
        let encoded = name.as_bytes();
        if encoded.len() > IR_NAME_LEN {
            return Err(PatchError::Ir("IR name exceeds 32 bytes.".into()));
        }
        self.data[IR_FLAG_OFFSET] = 1;
        self.data[0x83] = 0;
        self.data[0x84] = 0;
        self.data[0x85] = 0;
        for index in 0..IR_NAME_LEN {
            self.data[IR_NAME_OFFSET + index] = encoded.get(index).copied().unwrap_or(0);
        }
        self.data[IR_WAV_OFFSET..self.profile.record_size].copy_from_slice(wav);
        Ok(())
    }

    /// Usuwa IR (zeruje region `0x82..record_size`).
    pub fn clear_ir(&mut self) {
        for b in &mut self.data[IR_FLAG_OFFSET..self.profile.record_size] {
            *b = 0;
        }
    }

    /// Różnice bajtowe względem oryginału (offset/before/after) — port `differences`.
    pub fn differences(&self, original: &PatchRecord) -> Vec<ByteDifference> {
        original
            .data
            .iter()
            .zip(self.data.iter())
            .enumerate()
            .filter_map(|(offset, (&before, &after))| {
                if before == after {
                    None
                } else {
                    Some(ByteDifference {
                        offset,
                        before,
                        after,
                    })
                }
            })
            .collect()
    }
}

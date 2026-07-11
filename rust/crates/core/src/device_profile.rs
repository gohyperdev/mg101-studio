//! Profil urządzenia — port `DeviceProfile` + `ProfileLoader.validate` ze Swift.
//!
//! Profil to DANE (JSON): opis układu pamięci bez zaszytego kodu urządzenia
//! (ADR-0002). Klucze JSON są w camelCase → `rename_all = "camelCase"`.

use crate::effect_catalog::EffectCatalog;
use crate::error::ProfileError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Zakres bajtów (offset + długość).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub offset: usize,
    pub length: usize,
}

/// Konfiguracja BPM (dwa bajty 7-bitowe MSB/LSB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bpm {
    pub msb_offset: usize,
    pub lsb_offset: usize,
    pub minimum: i64,
    pub maximum: i64,
}

/// Blok łańcucha sygnału (selektor modelu + offsety parametrów).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    pub display_name: String,
    pub selector_offset: usize,
    pub parameter_offsets: Vec<usize>,
}

/// Pole globalne patcha (send/return/patch.*).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedField {
    pub offset: usize,
    pub minimum: i64,
    pub maximum: i64,
}

/// Profil urządzenia (deklaratywny opis układu rekordu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProfile {
    pub schema_version: i64,
    pub id: String,
    pub display_name: String,
    pub record_size: usize,
    pub patch_name: ByteRange,
    pub bpm: Bpm,
    pub blocks: Vec<Block>,
    /// Uwaga: mapa uporządkowana dla determinizmu iteracji.
    pub named_fields: BTreeMap<String, NamedField>,
}

impl DeviceProfile {
    /// Zwraca blok o danym id lub błąd.
    pub fn block(&self, id: &str) -> Result<&Block, ProfileError> {
        self.blocks
            .iter()
            .find(|b| b.id == id)
            .ok_or_else(|| ProfileError::UnknownBlock(id.to_string()))
    }

    /// Parsuje profil z JSON.
    pub fn from_json(s: &str) -> Result<Self, ProfileError> {
        serde_json::from_str(s).map_err(|e| ProfileError::Malformed(e.to_string()))
    }

    /// Walidacja profilu i katalogu — wierny port `ProfileLoader.validate`.
    pub fn validate(&self, catalog: &EffectCatalog) -> Result<(), ProfileError> {
        if self.schema_version != 1 {
            return Err(ProfileError::Malformed(format!(
                "Unsupported schemaVersion {}; expected 1.",
                self.schema_version
            )));
        }
        if self.record_size == 0 {
            return Err(ProfileError::Malformed(
                "recordSize must be positive.".into(),
            ));
        }
        let require_offset = |offset: usize, label: &str| -> Result<(), ProfileError> {
            if offset < self.record_size {
                Ok(())
            } else {
                Err(ProfileError::Malformed(format!(
                    "{label} offset {offset} is outside the record."
                )))
            }
        };
        if self.patch_name.length == 0
            || self.patch_name.offset + self.patch_name.length > self.record_size
        {
            return Err(ProfileError::Malformed(
                "patchName range is outside the record.".into(),
            ));
        }
        require_offset(self.bpm.msb_offset, "bpm.msb")?;
        require_offset(self.bpm.lsb_offset, "bpm.lsb")?;
        if self.bpm.minimum > self.bpm.maximum {
            return Err(ProfileError::Malformed("BPM range is reversed.".into()));
        }

        let mut block_ids = std::collections::HashSet::new();
        let mut selector_offsets = std::collections::HashSet::new();
        for block in &self.blocks {
            if !block_ids.insert(block.id.clone()) {
                return Err(ProfileError::Malformed(format!(
                    "Duplicate block id {}.",
                    block.id
                )));
            }
            if !selector_offsets.insert(block.selector_offset) {
                return Err(ProfileError::Malformed(format!(
                    "Duplicate selector offset {}.",
                    block.selector_offset
                )));
            }
            require_offset(block.selector_offset, &format!("{}.selector", block.id))?;
            for &offset in &block.parameter_offsets {
                require_offset(offset, &format!("{}.parameter", block.id))?;
            }
        }
        for (name, field) in &self.named_fields {
            require_offset(field.offset, name)?;
            if field.minimum > field.maximum
                || !(0..=255).contains(&field.minimum)
                || !(0..=255).contains(&field.maximum)
            {
                return Err(ProfileError::Malformed(format!(
                    "Invalid range for field {name}."
                )));
            }
        }

        for (block_id, module) in &catalog.modules {
            let Some(block) = self.blocks.iter().find(|b| &b.id == block_id) else {
                return Err(ProfileError::Malformed(format!(
                    "Catalog module {block_id} has no profile block."
                )));
            };
            let mut model_ids = std::collections::HashSet::new();
            for (key, model) in &module.models {
                if key != &model.model_id.to_string() || !(0..=63).contains(&model.model_id) {
                    return Err(ProfileError::Malformed(format!(
                        "Invalid model key/id {block_id}.{key}."
                    )));
                }
                if !model_ids.insert(model.model_id) {
                    return Err(ProfileError::Malformed(format!(
                        "Duplicate model id {block_id}.{}.",
                        model.model_id
                    )));
                }
                let mut parameter_offsets = std::collections::HashSet::new();
                for parameter in &model.parameters {
                    if parameter.raw_range.len() != 2
                        || !(0..=255).contains(&parameter.minimum())
                        || !(0..=255).contains(&parameter.maximum())
                        || parameter.minimum() > parameter.maximum()
                    {
                        return Err(ProfileError::Malformed(format!(
                            "Invalid range for {block_id}.{}.{}.",
                            model.model_id, parameter.name
                        )));
                    }
                    if !block.parameter_offsets.contains(&parameter.file_offset) {
                        return Err(ProfileError::Malformed(format!(
                            "Parameter {block_id}.{}.{} uses offset {} outside its block.",
                            model.model_id, parameter.name, parameter.file_offset
                        )));
                    }
                    if !parameter_offsets.insert(parameter.file_offset) {
                        return Err(ProfileError::Malformed(format!(
                            "Duplicate parameter offset in {block_id}.{}.",
                            model.model_id
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

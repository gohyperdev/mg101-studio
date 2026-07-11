//! Warstwa aplikacyjna — wykonawca komend (port `StudioState` + `GUIToolExecutor`).
//!
//! Spina rejestr komend (E3) z edycją kanoniczną (core `PatchRecord`, sterowaną
//! profilem/katalogiem — DANE) i składem Biblioteki (E4). Ten sam wykonawca
//! obsłuży agenta (E6), MCP i UI (E7) — jedno źródło zachowania (ADR-0002).
//!
//! Device-agnostyczny: żadnej wiedzy o MG-101 w kodzie — profil i katalog są
//! wstrzykiwane. `blob` patcha to edytowalny rekord urządzenia o `record_size`
//! z profilu.
//!
//! E6.2: narzędzia odczytu + edycji + duplikat/select. Filesystem (import/export/
//! set_ir z pliku/list_files) i revert/WAL wchodzą w E6.2b.

use mg101_commands::{Command, TargetRef};
use mg101_core::{DeviceProfile, EffectCatalog, PatchError, PatchRecord, ProfileError};
use mg101_library::{LibraryError, LibraryPatch, LibraryStore, PatchId};
use serde_json::{json, Value};

/// Błąd wykonania narzędzia (port `ToolExecutionError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    NotFound(PatchId),
    Conflict { current_revision: u64 },
    Unsupported(String),
    Patch(String),
    Library(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::NotFound(id) => write!(f, "Nie znaleziono patcha: {id}"),
            ExecError::Conflict { current_revision } => {
                write!(
                    f,
                    "Konflikt współbieżności: bieżąca rewizja to {current_revision}"
                )
            }
            ExecError::Unsupported(m) => write!(f, "Nieobsługiwane: {m}"),
            ExecError::Patch(m) => write!(f, "Błąd patcha: {m}"),
            ExecError::Library(m) => write!(f, "Błąd biblioteki: {m}"),
        }
    }
}

impl From<PatchError> for ExecError {
    fn from(e: PatchError) -> Self {
        ExecError::Patch(e.to_string())
    }
}
impl From<ProfileError> for ExecError {
    fn from(e: ProfileError) -> Self {
        ExecError::Unsupported(e.to_string())
    }
}
impl From<LibraryError> for ExecError {
    fn from(e: LibraryError) -> Self {
        match e {
            LibraryError::NotFound(id) => ExecError::NotFound(id),
            LibraryError::RevisionConflict { actual, .. } => ExecError::Conflict {
                current_revision: actual,
            },
            other => ExecError::Library(other.to_string()),
        }
    }
}

/// Wykonawca komend nad Biblioteką + edycją kanoniczną.
pub struct Studio<'p, S: LibraryStore> {
    store: S,
    profile: &'p DeviceProfile,
    catalog: &'p EffectCatalog,
    selected_patch: Option<PatchId>,
    selected_block: String,
    /// Licznik do deterministycznego nadawania id (duplikaty).
    seq: u64,
    /// Znacznik czasu dostarczany przez wywołującego (wasm-safe).
    now_ms: i64,
}

impl<'p, S: LibraryStore> Studio<'p, S> {
    /// Nowy wykonawca. `selected_block` domyślnie pierwszy blok profilu.
    pub fn new(
        store: S,
        profile: &'p DeviceProfile,
        catalog: &'p EffectCatalog,
        now_ms: i64,
    ) -> Self {
        let selected_block = profile
            .blocks
            .first()
            .map(|b| b.id.clone())
            .unwrap_or_default();
        Self {
            store,
            profile,
            catalog,
            selected_patch: None,
            selected_block,
            seq: 0,
            now_ms,
        }
    }

    /// Dostęp do składu (dla testów / warstwy wyżej).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Aktualizuje znacznik czasu operacji.
    pub fn set_now(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
    }

    /// Dekoduje blob patcha na `PatchRecord` (sterowany profilem).
    fn record(&self, patch: &LibraryPatch) -> Result<PatchRecord<'p>, ExecError> {
        Ok(PatchRecord::new(patch.blob.clone(), self.profile)?)
    }

    fn get_patch(&self, id: &PatchId) -> Result<LibraryPatch, ExecError> {
        self.store
            .get(id)
            .ok_or_else(|| ExecError::NotFound(id.clone()))
    }

    /// Wykonuje komendę, zwracając JSON wyniku (kształt 1:1 z v1).
    pub fn execute(&mut self, command: &Command) -> Result<Value, ExecError> {
        match command {
            Command::ListPatches => self.list_patches(),
            Command::GetPatch { patch_id } => self.get_patch_json(patch_id),
            Command::GetSelection => Ok(self.get_selection()),
            Command::ListModels { block } => Ok(self.list_models(block)),
            Command::GetProfile => Ok(self.get_profile()),
            Command::GetDiff { patch_id } => self.get_diff(patch_id),
            Command::SetParameter {
                target,
                block,
                parameter,
                value,
            } => self.mutate(target, |rec, prof, cat| {
                let blk = prof.block(block)?;
                let model_id = rec.model_id(blk);
                let model = cat.model(block, model_id).ok_or_else(|| {
                    ExecError::Unsupported(format!("nieznany model {block}.{model_id}"))
                })?;
                let param = model
                    .parameters
                    .iter()
                    .find(|p| &p.name == parameter)
                    .ok_or_else(|| {
                        ExecError::Unsupported(format!("nieznany parametr {block}.{parameter}"))
                    })?;
                rec.set_parameter(param, *value)?;
                Ok(())
            }),
            Command::SetModel {
                target,
                block,
                model,
                values,
                bypassed,
            } => self.mutate(target, |rec, prof, cat| {
                let blk = prof.block(block)?;
                let m = cat.model(block, *model).ok_or_else(|| {
                    ExecError::Unsupported(format!("nieznany model {block}.{model}"))
                })?;
                // clear_inactive=true — model określa aktywne offsety (parytet v1).
                rec.set_model(m, blk, values, *bypassed, true)?;
                Ok(())
            }),
            Command::SetBypass {
                target,
                block,
                bypassed,
            } => self.mutate(target, |rec, prof, _| {
                let blk = prof.block(block)?;
                rec.set_bypass(*bypassed, blk);
                Ok(())
            }),
            Command::SetName { target, name } => self.mutate(target, |rec, _, _| {
                rec.set_name(name)?;
                Ok(())
            }),
            Command::SetBpm { target, bpm } => self.mutate(target, |rec, _, _| {
                rec.set_bpm(*bpm)?;
                Ok(())
            }),
            Command::SetNamedField {
                target,
                field,
                value,
            } => self.mutate(target, |rec, _, _| {
                rec.set_named_field(field, *value)?;
                Ok(())
            }),
            Command::ClearIr { target } => self.mutate(target, |rec, _, _| {
                rec.clear_ir();
                Ok(())
            }),
            Command::DuplicatePatch { target } => self.duplicate(target),
            Command::SelectPatch { patch_id } => self.select(patch_id),
            other => Err(ExecError::Unsupported(format!(
                "narzędzie {} wchodzi w E6.2b (filesystem/revert)",
                other.tool_name()
            ))),
        }
    }

    // --- Odczyt ---

    fn list_patches(&self) -> Result<Value, ExecError> {
        let mut out = Vec::new();
        for (i, p) in self.store.all().iter().enumerate() {
            let name = self.record(p).map(|r| r.name()).unwrap_or_default();
            out.push(json!({
                "patchID": p.id,
                "slot": i + 1,
                "name": name,
                "origin": format!("{:?}", p.origin),
                "revision": p.revision,
            }));
        }
        Ok(Value::Array(out))
    }

    fn get_patch_json(&self, id: &PatchId) -> Result<Value, ExecError> {
        let item = self.get_patch(id)?;
        let rec = self.record(&item)?;
        let blocks: Vec<Value> = self
            .profile
            .blocks
            .iter()
            .map(|block| {
                let model_id = rec.model_id(block);
                let current = self.catalog.model(&block.id, model_id);
                let parameters: Vec<Value> = current
                    .map(|m| {
                        m.parameters
                            .iter()
                            .map(|param| {
                                json!({
                                    "name": param.name,
                                    "value": rec.value_at(param.file_offset),
                                    "minimum": param.minimum(),
                                    "maximum": param.maximum(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "block": block.id,
                    "current_model": model_id,
                    "current_model_name": current.map(|m| m.display_name.clone()).unwrap_or_else(|| "unknown".into()),
                    "bypassed": rec.is_bypassed(block),
                    "parameters": parameters,
                })
            })
            .collect();
        let named_fields: serde_json::Map<String, Value> = self
            .profile
            .named_fields
            .iter()
            .map(|(k, field)| {
                (
                    k.clone(),
                    json!({
                        "value": rec.value_at(field.offset),
                        "minimum": field.minimum,
                        "maximum": field.maximum,
                    }),
                )
            })
            .collect();
        Ok(json!({
            "patchID": item.id,
            "name": rec.name(),
            "bpm": rec.bpm(),
            "revision": item.revision,
            "ir": {"present": rec.ir_present(), "name": rec.ir_name()},
            "blocks": blocks,
            "named_fields": named_fields,
        }))
    }

    fn get_selection(&self) -> Value {
        json!({
            "selectedPatchID": self.selected_patch.clone().map(Value::String).unwrap_or(Value::Null),
            "selectedBlockID": self.selected_block,
        })
    }

    fn list_models(&self, block: &str) -> Value {
        let list: Vec<Value> = self
            .catalog
            .models(block)
            .iter()
            .map(|model| {
                json!({
                    "modelID": model.model_id,
                    "displayName": model.display_name,
                    "parameters": model.parameters.iter().map(|p| json!({
                        "name": p.name,
                        "minimum": p.minimum(),
                        "maximum": p.maximum(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        Value::Array(list)
    }

    fn get_profile(&self) -> Value {
        let blocks: Vec<Value> = self
            .profile
            .blocks
            .iter()
            .map(|b| json!({"id": b.id, "displayName": b.display_name}))
            .collect();
        let named_fields: serde_json::Map<String, Value> = self
            .profile
            .named_fields
            .iter()
            .map(|(k, f)| {
                (
                    k.clone(),
                    json!({"minimum": f.minimum, "maximum": f.maximum}),
                )
            })
            .collect();
        json!({
            "recordSize": self.profile.record_size,
            "blocks": blocks,
            "namedFields": named_fields,
        })
    }

    fn get_diff(&self, id: &PatchId) -> Result<Value, ExecError> {
        let item = self.get_patch(id)?;
        let rec = self.record(&item)?;
        // Bazą różnicy jest bieżący blob względem "czystego" rekordu tej samej
        // długości? v1 porównywał z importowanym originałem; tu brak baseline w
        // Bibliotece → różnice względem zer (TODO baseline w E6.2b/BACKLOG).
        let baseline = PatchRecord::new(vec![0u8; item.blob.len()], self.profile)?;
        let diffs: Vec<Value> = rec
            .differences(&baseline)
            .iter()
            .map(|d| json!({"offset": d.offset, "before": d.before, "after": d.after}))
            .collect();
        Ok(Value::Array(diffs))
    }

    // --- Mutacje ---

    /// Wspólna ścieżka mutacji: kontrola celu (tylko Library), rewizji, dekod →
    /// edycja → re-enkod → zapis z bumpem rewizji (E4 optimistic concurrency).
    fn mutate<F>(&mut self, target: &TargetRef, edit: F) -> Result<Value, ExecError>
    where
        F: FnOnce(&mut PatchRecord, &DeviceProfile, &EffectCatalog) -> Result<(), ExecError>,
    {
        let (patch_id, revision) = library_target(target)?;
        let mut item = self.get_patch(&patch_id)?;
        let mut rec = self.record(&item)?;
        edit(&mut rec, self.profile, self.catalog)?;
        item.blob = rec.data().to_vec();
        item.updated_at = self.now_ms;
        let new_rev = self.store.update(item, revision)?;
        Ok(json!({"success": true, "revision": new_rev}))
    }

    fn duplicate(&mut self, target: &TargetRef) -> Result<Value, ExecError> {
        let (patch_id, revision) = library_target(target)?;
        let src = self.get_patch(&patch_id)?;
        if src.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: src.revision,
            });
        }
        self.seq += 1;
        let new_id = format!("{patch_id}-copy-{}", self.seq);
        let mut copy = src.clone();
        copy.id = new_id.clone();
        copy.origin = mg101_library::PatchOrigin::Created;
        copy.revision = 1;
        copy.created_at = self.now_ms;
        copy.updated_at = self.now_ms;
        copy.tags.clear();
        copy.groups.clear();
        self.store.add(copy)?;
        Ok(json!({"success": true, "newPatchID": new_id}))
    }

    fn select(&mut self, id: &PatchId) -> Result<Value, ExecError> {
        if self.store.get(id).is_none() {
            return Err(ExecError::NotFound(id.clone()));
        }
        self.selected_patch = Some(id.clone());
        Ok(json!({"success": true}))
    }
}

/// Wymusza cel biblioteczny (executor nie pisze plików — to warstwa wyżej).
fn library_target(target: &TargetRef) -> Result<(PatchId, u64), ExecError> {
    match target {
        TargetRef::Library {
            patch_id,
            expected_revision,
        } => Ok((patch_id.clone(), *expected_revision as u64)),
        TargetRef::File { .. } => Err(ExecError::Unsupported(
            "wykonawca wymaga celu bibliotecznego (pliki: E6.2b/MCP)".into(),
        )),
    }
}

#[cfg(test)]
mod tests;

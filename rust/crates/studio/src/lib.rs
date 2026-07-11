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
//! Komplet narzędzi v1: odczyt + edycja + duplikat/select (E6.2); WAL sesji
//! (mutate/duplicate/delete prepared→committed), revert_last/revert_session przez
//! inwersy, oraz filesystem set_ir/list_files/import/export (E6.2b, natywne).

#[cfg(not(target_arch = "wasm32"))]
pub mod file_ops;

use mg101_commands::{Command, TargetRef};
use mg101_core::wal::{
    last_committed, sha256_hex, EntryState, InverseOperation, JournalStore, StagedMetadata,
    TransactionEntry,
};
use mg101_core::{DeviceProfile, EffectCatalog, PatchError, PatchRecord, ProfileError};
use mg101_library::{
    content_hash, LibraryError, LibraryPatch, LibraryStore, MaskRange, PatchId, PatchOrigin,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
    /// Dziennik WAL sesji (opcjonalny) — mutacje prepared→committed, revert.
    journal: Option<Box<dyn JournalStore>>,
    /// Poczekalnia soft-delete (dla RestoreFromStaging przy revert).
    staging: BTreeMap<PatchId, LibraryPatch>,
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
            journal: None,
            staging: BTreeMap::new(),
        }
    }

    /// Podłącza dziennik WAL (mutacje journalowane, revert dostępny).
    pub fn with_journal(mut self, journal: Box<dyn JournalStore>) -> Self {
        self.journal = Some(journal);
        self
    }

    /// Czyści zaznaczenie, jeśli wskazuje na patch, którego już nie ma
    /// (po delete/revert) — inaczej get_selection zwróciłby martwe ID (review E6/W4).
    fn fix_selection(&mut self) {
        if let Some(id) = &self.selected_patch {
            if self.store.get(id).is_none() {
                self.selected_patch = None;
            }
        }
    }

    /// Dostęp do składu (dla testów / warstwy wyżej).
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Region nazwy patcha jako maska fingerprintu (z profilu — dane).
    fn name_mask(&self) -> [MaskRange; 1] {
        [MaskRange {
            start: self.profile.patch_name.offset,
            len: self.profile.patch_name.length,
        }]
    }

    /// Przelicza fingerprint patcha po edycji blobu (spójność z silnikiem sync).
    fn refresh_hashes(&self, patch: &mut LibraryPatch) {
        patch.exact_hash = sha256_hex(&patch.blob);
        patch.content_hash = content_hash(&patch.blob, &self.name_mask());
    }

    /// Kolejny numer sekwencji WAL (liczba wpisów, jak v1).
    fn next_wal_seq(&self) -> u64 {
        self.journal
            .as_ref()
            .and_then(|j| j.load().ok())
            .map(|e| e.len() as u64)
            .unwrap_or(0)
    }

    fn journal_append(&self, entry: &TransactionEntry) -> Result<(), ExecError> {
        if let Some(j) = &self.journal {
            j.append(entry)
                .map_err(|e| ExecError::Library(e.to_string()))?;
        }
        Ok(())
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
            Command::DeletePatch { target } => self.delete(target),
            Command::RevertLastAgentAction => self.revert_last(),
            Command::RevertSession => self.revert_session(),
            Command::SetIr {
                target,
                wav_path,
                name,
            } => self.set_ir(target, wav_path, name),
            Command::ImportPatch { path } => self.import_patch(path),
            Command::ExportPatch {
                target,
                destination_path,
            } => self.export_patch(target, destination_path),
            Command::ListFiles { path } => self.list_files(path),
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
                                    "label": param.label(),
                                    "value": rec.value_at(param.file_offset),
                                    "minimum": param.minimum(),
                                    "maximum": param.maximum(),
                                    "control": param.control().as_str(),
                                    "unit": param.unit,
                                    "midi_cc": param.midi_cc,
                                    "confirmed": param.is_confirmed(),
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
        // Kontrola rewizji PRZED wpisem WAL (review E6/K2): konflikt nie może
        // zostawić osieroconego wpisu `prepared`, który zatruje revert_session.
        if item.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: item.revision,
            });
        }
        let before_blob = item.blob.clone();
        let before_hash = sha256_hex(&before_blob);

        let mut rec = self.record(&item)?;
        edit(&mut rec, self.profile, self.catalog)?;
        let new_blob = rec.data().to_vec();

        // Brak realnej zmiany → nic nie robimy (po kontroli rewizji, parytet v1).
        if new_blob == before_blob {
            return Ok(json!({"success": true, "revision": item.revision}));
        }

        item.blob = new_blob;
        item.updated_at = self.now_ms;
        self.refresh_hashes(&mut item);
        let after_hash = item.exact_hash.clone();

        // WAL: prepared (inverse = przywróć poprzednie bajty).
        let seq = self.next_wal_seq();
        let mut entry = TransactionEntry {
            sequence: seq,
            timestamp_ms: self.now_ms,
            tool_name: "mutate".into(),
            patch_id: patch_id.clone(),
            revision_before: revision as i64,
            inverse: InverseOperation::RestoreBytes {
                patch_id: patch_id.clone(),
                blob: before_blob,
            },
            before_hash,
            after_hash,
            state: EntryState::Prepared,
        };
        self.journal_append(&entry)?;

        let new_rev = self.store.update(item, revision)?;

        entry.state = EntryState::Committed;
        self.journal_append(&entry)?;
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
        copy.origin = PatchOrigin::Created;
        copy.revision = 1;
        copy.created_at = self.now_ms;
        copy.updated_at = self.now_ms;
        copy.tags.clear();
        copy.groups.clear();
        self.refresh_hashes(&mut copy);
        let after_hash = copy.exact_hash.clone();

        // WAL: prepared (inverse = usuń nowy patch).
        let seq = self.next_wal_seq();
        let mut entry = TransactionEntry {
            sequence: seq,
            timestamp_ms: self.now_ms,
            tool_name: "duplicate".into(),
            patch_id: new_id.clone(),
            revision_before: 0,
            inverse: InverseOperation::RemovePatch {
                patch_id: new_id.clone(),
            },
            before_hash: String::new(),
            after_hash,
            state: EntryState::Prepared,
        };
        self.journal_append(&entry)?;

        self.store.add(copy)?;

        entry.state = EntryState::Committed;
        self.journal_append(&entry)?;
        Ok(json!({"success": true, "newPatchID": new_id}))
    }

    fn delete(&mut self, target: &TargetRef) -> Result<Value, ExecError> {
        let (patch_id, revision) = library_target(target)?;
        let item = self.get_patch(&patch_id)?;
        if item.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: item.revision,
            });
        }
        let before_hash = sha256_hex(&item.blob);
        let meta = StagedMetadata {
            origin: format!("{:?}", item.origin),
            source_name: item.name.clone(),
            revision: item.revision as i64,
            file_name: format!("{patch_id}.mg101patch"),
        };

        // WAL: prepared (inverse = przywróć z poczekalni).
        let seq = self.next_wal_seq();
        let mut entry = TransactionEntry {
            sequence: seq,
            timestamp_ms: self.now_ms,
            tool_name: "delete".into(),
            patch_id: patch_id.clone(),
            revision_before: item.revision as i64,
            inverse: InverseOperation::RestoreFromStaging {
                patch_id: patch_id.clone(),
                meta,
            },
            before_hash,
            after_hash: String::new(),
            state: EntryState::Prepared,
        };
        self.journal_append(&entry)?;

        let removed = self.store.remove(&patch_id)?;
        self.staging.insert(patch_id.clone(), removed); // poczekalnia dla revert

        entry.state = EntryState::Committed;
        self.journal_append(&entry)?;
        self.fix_selection();
        Ok(json!({"success": true}))
    }

    fn revert_last(&mut self) -> Result<Value, ExecError> {
        let entries = self.load_journal()?;
        let last = last_committed(&entries)
            .cloned()
            .ok_or_else(|| ExecError::Unsupported("brak akcji do cofnięcia".into()))?;
        self.apply_inverse(&last.inverse, &last.after_hash)?;
        // Usuń wpisy tej sekwencji z dziennika (jak v1).
        let filtered: Vec<TransactionEntry> = entries
            .into_iter()
            .filter(|e| e.sequence != last.sequence)
            .collect();
        self.rewrite_journal(&filtered)?;
        self.fix_selection();
        Ok(json!({"success": true}))
    }

    fn revert_session(&mut self) -> Result<Value, ExecError> {
        let entries = self.load_journal()?;
        // Wpisy do cofnięcia: committed oraz zawieszone prepared; najnowsze pierwsze
        // (kolejność jak session_inverses, ale z zachowaniem after_hash per wpis).
        let committed_seqs: std::collections::HashSet<u64> = entries
            .iter()
            .filter(|e| e.state == EntryState::Committed)
            .map(|e| e.sequence)
            .collect();
        let mut to_revert: Vec<&TransactionEntry> = entries
            .iter()
            .filter(|e| {
                e.state == EntryState::Committed
                    || (e.state == EntryState::Prepared && !committed_seqs.contains(&e.sequence))
            })
            .collect();
        to_revert.sort_by(|a, b| b.sequence.cmp(&a.sequence));
        let plan: Vec<(InverseOperation, String)> = to_revert
            .into_iter()
            .map(|e| (e.inverse.clone(), e.after_hash.clone()))
            .collect();
        for (inv, after_hash) in &plan {
            self.apply_inverse(inv, after_hash)?;
        }
        self.rewrite_journal(&[])?;
        self.fix_selection();
        Ok(json!({"success": true}))
    }

    /// Stosuje operację odwrotną do składu Biblioteki (port `applyInverse`).
    fn apply_inverse(
        &mut self,
        inverse: &InverseOperation,
        after_hash: &str,
    ) -> Result<(), ExecError> {
        match inverse {
            InverseOperation::RestoreBytes { patch_id, blob } => {
                let mut item = self
                    .store
                    .get(patch_id)
                    .ok_or_else(|| ExecError::NotFound(patch_id.clone()))?;
                if sha256_hex(&item.blob) != after_hash {
                    return Err(ExecError::Unsupported(format!(
                        "konflikt cofania: patch {patch_id} ma ręczne zmiany"
                    )));
                }
                item.blob = blob.clone();
                self.refresh_hashes(&mut item);
                // Zapis z bieżącą rewizją (bez kontroli — revert autorytatywny).
                let current = item.revision;
                self.store.update(item, current)?;
                Ok(())
            }
            InverseOperation::RemovePatch { patch_id } => {
                if let Some(existing) = self.store.get(patch_id) {
                    if sha256_hex(&existing.blob) != after_hash {
                        return Err(ExecError::Unsupported(format!(
                            "konflikt cofania: duplikat {patch_id} ma ręczne zmiany"
                        )));
                    }
                    self.store.remove(patch_id)?;
                }
                Ok(())
            }
            InverseOperation::RestoreFromStaging { patch_id, .. } => {
                if self.store.get(patch_id).is_some() {
                    return Err(ExecError::Unsupported(format!(
                        "konflikt cofania: patch {patch_id} już istnieje"
                    )));
                }
                let staged = self
                    .staging
                    .remove(patch_id)
                    .ok_or_else(|| ExecError::NotFound(patch_id.clone()))?;
                self.store.add(staged)?;
                Ok(())
            }
        }
    }

    fn load_journal(&self) -> Result<Vec<TransactionEntry>, ExecError> {
        self.journal
            .as_ref()
            .ok_or_else(|| ExecError::Unsupported("brak aktywnej sesji/dziennika".into()))?
            .load()
            .map_err(|e| ExecError::Library(e.to_string()))
    }

    fn rewrite_journal(&self, entries: &[TransactionEntry]) -> Result<(), ExecError> {
        if let Some(j) = &self.journal {
            j.rewrite(entries)
                .map_err(|e| ExecError::Library(e.to_string()))?;
        }
        Ok(())
    }

    fn select(&mut self, id: &PatchId) -> Result<Value, ExecError> {
        if self.store.get(id).is_none() {
            return Err(ExecError::NotFound(id.clone()));
        }
        self.selected_patch = Some(id.clone());
        Ok(json!({"success": true}))
    }

    // --- Filesystem (natywne; na wasm zwracają Unsupported) ---

    #[cfg(not(target_arch = "wasm32"))]
    fn set_ir(
        &mut self,
        target: &TargetRef,
        wav_path: &str,
        name: &str,
    ) -> Result<Value, ExecError> {
        let wav = std::fs::read(wav_path)
            .map_err(|e| ExecError::Unsupported(format!("odczyt WAV '{wav_path}': {e}")))?;
        let name = name.to_string();
        self.mutate(target, move |rec, _, _| {
            rec.set_ir(&wav, &name)?;
            Ok(())
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn list_files(&self, path: &str) -> Result<Value, ExecError> {
        let entries = std::fs::read_dir(path)
            .map_err(|e| ExecError::Unsupported(format!("katalog '{path}': {e}")))?;
        let mut files = Vec::new();
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if !name.starts_with('.') {
                    files.push(Value::String(name.to_string()));
                }
            }
        }
        Ok(json!({"success": true, "files": files}))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn import_patch(&mut self, path: &str) -> Result<Value, ExecError> {
        let data = std::fs::read(path)
            .map_err(|e| ExecError::Unsupported(format!("odczyt '{path}': {e}")))?;
        let rs = self.profile.record_size;
        if data.is_empty() || !data.len().is_multiple_of(rs) {
            return Err(ExecError::Unsupported(format!(
                "plik nie jest wielokrotnością rekordu {rs} B (rozmiar {})",
                data.len()
            )));
        }
        let mut ids = Vec::new();
        for (i, chunk) in data.chunks(rs).enumerate() {
            self.seq += 1;
            let id = format!("import-{}-{}", self.seq, i);
            // Walidacja: rekord musi się dekodować profilem (bajty święte zachowane).
            PatchRecord::new(chunk.to_vec(), self.profile)?;
            let blob = chunk.to_vec();
            let mut patch = LibraryPatch {
                id: id.clone(),
                name: String::new(),
                content_hash: String::new(),
                exact_hash: String::new(),
                blob,
                origin: PatchOrigin::ImportedFile,
                device_id: self.profile.id.clone(),
                firmware: None,
                codec_version: "1".into(),
                tags: Default::default(),
                groups: Default::default(),
                created_at: self.now_ms,
                updated_at: self.now_ms,
                revision: 1,
            };
            patch.name = PatchRecord::new(patch.blob.clone(), self.profile)?.name();
            self.refresh_hashes(&mut patch);
            self.store.add(patch)?;
            ids.push(Value::String(id));
        }
        Ok(json!({"success": true, "importedPatchIDs": ids}))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn export_patch(
        &mut self,
        target: &TargetRef,
        destination_path: &str,
    ) -> Result<Value, ExecError> {
        let (patch_id, revision) = library_target(target)?;
        let item = self.get_patch(&patch_id)?;
        if item.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: item.revision,
            });
        }
        // Jeśli cel to katalog — dołóż nazwę pliku z nazwy patcha.
        let dest = std::path::Path::new(destination_path);
        let final_path = if dest.is_dir() {
            let name = item.name.trim();
            let filename = if name.is_empty() {
                patch_id.as_str()
            } else {
                name
            };
            dest.join(format!("{filename}.mg101patch"))
        } else {
            dest.to_path_buf()
        };
        std::fs::write(&final_path, &item.blob).map_err(|e| {
            ExecError::Unsupported(format!("zapis '{}': {e}", final_path.display()))
        })?;
        Ok(json!({"success": true, "path": final_path.display().to_string()}))
    }

    // Warianty wasm — operacje plikowe niedostępne w przeglądarce.
    #[cfg(target_arch = "wasm32")]
    fn set_ir(&mut self, _t: &TargetRef, _w: &str, _n: &str) -> Result<Value, ExecError> {
        Err(ExecError::Unsupported(
            "filesystem niedostępny na wasm".into(),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    fn list_files(&self, _p: &str) -> Result<Value, ExecError> {
        Err(ExecError::Unsupported(
            "filesystem niedostępny na wasm".into(),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    fn import_patch(&mut self, _p: &str) -> Result<Value, ExecError> {
        Err(ExecError::Unsupported(
            "filesystem niedostępny na wasm".into(),
        ))
    }
    #[cfg(target_arch = "wasm32")]
    fn export_patch(&mut self, _t: &TargetRef, _d: &str) -> Result<Value, ExecError> {
        Err(ExecError::Unsupported(
            "filesystem niedostępny na wasm".into(),
        ))
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

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

    /// Wstawia patch pobrany z urządzenia (już zdekodowany do rekordu plikowego)
    /// pod stabilnym `id`. Idempotentne: gdy `id` już istnieje, nie nadpisuje
    /// (ponowne kliknięcie slotu wybierze istniejący patch, nie zdubluje edycji).
    /// Warstwa device-agnostyczna: przyjmuje bajty rekordu, nie wie o kodeku wire.
    pub fn insert_device_patch(
        &mut self,
        id: &str,
        name: &str,
        blob: Vec<u8>,
    ) -> Result<(), ExecError> {
        if self.store.get(&id.to_string()).is_some() {
            return Ok(());
        }
        let content_hash = content_hash(&blob, &self.name_mask());
        let exact_hash = mg101_library::exact_hash(&blob);
        self.store.add(LibraryPatch {
            id: id.to_string(),
            name: name.to_string(),
            baseline_blob: blob.clone(),
            blob,
            origin: PatchOrigin::PulledFromDevice,
            device_id: self.profile.id.clone(),
            firmware: None,
            codec_version: "wire-v1".into(),
            content_hash,
            exact_hash,
            tags: Default::default(),
            groups: Default::default(),
            meta: Default::default(),
            created_at: self.now_ms,
            updated_at: self.now_ms,
            revision: 1,
        })?;
        Ok(())
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
            Command::GetRaw { patch_id } => self.get_raw(patch_id),
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
            // Metadane i kolekcje.
            Command::SetPatchMeta { target, meta } => self.set_patch_meta(target, meta),
            Command::AddTag { target, tag } => self.change_tag(target, tag, true),
            Command::RemoveTag { target, tag } => self.change_tag(target, tag, false),
            Command::ListCollections => Ok(self.list_collections()),
            Command::CreateCollection { name } => self.create_collection(name),
            Command::DeleteCollection { collection } => self.delete_collection(collection),
            Command::AddToCollection { target, collection } => {
                self.change_collection(target, collection, true)
            }
            Command::RemoveFromCollection { target, collection } => {
                self.change_collection(target, collection, false)
            }
            // Katalog publicznych źródeł jest danymi warstwy urządzenia (desktop) —
            // Studio nie zna konkretnego modelu (ADR-0002).
            Command::ListPatchSources => Err(ExecError::Unsupported(
                "katalog źródeł dostarcza warstwa aplikacji".into(),
            )),
            // Sterowanie DRUM na żywo obsługuje warstwa desktopu (MIDI CC/SysEx na
            // urządzenie) — nie modyfikuje patchy, więc Studio go nie realizuje.
            Command::DrumCatalog
            | Command::DrumTransport { .. }
            | Command::DrumVolume { .. }
            | Command::DrumPattern { .. }
            | Command::DrumTempo { .. } => Err(ExecError::Unsupported(
                "komenda DRUM wymaga podłączonego urządzenia (sterowanie na żywo)".into(),
            )),
        }
    }

    // --- Odczyt ---

    fn list_patches(&self) -> Result<Value, ExecError> {
        // Kolejność Biblioteki: alfabetycznie po nazwie patcha (a nie po technicznym
        // ID, jak zwraca magazyn `ORDER BY id` — to dawało „losową" kolejność:
        // import-<hash>, device-…, -copy-…). `slot` to pozycja w tym porządku.
        // Zakładki User/Factory idą osobno po indeksie slotu urządzenia (1A..9D).
        let mut items: Vec<(String, LibraryPatch)> = self
            .store
            .all()
            .into_iter()
            .map(|p| {
                let name = self.record(&p).map(|r| r.name()).unwrap_or_default();
                (name, p)
            })
            .collect();
        // Stabilnie: po nazwie (bez rozróżniania wielkości), remis rozstrzyga ID.
        items.sort_by(|a, b| {
            a.0.to_lowercase()
                .cmp(&b.0.to_lowercase())
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let out: Vec<Value> = items
            .into_iter()
            .enumerate()
            .map(|(i, (name, p))| {
                json!({
                    "patchID": p.id,
                    "slot": i + 1,
                    "name": name,
                    "origin": format!("{:?}", p.origin),
                    "revision": p.revision,
                    // Metadane w liście: pozwalają filtrować po kolekcji/ocenie i pokazać
                    // autora bez dociągania każdego patcha osobno.
                    "meta": meta_json(&p),
                    "tags": p.tags.iter().collect::<Vec<_>>(),
                    "collections": p.groups.iter().collect::<Vec<_>>(),
                })
            })
            .collect();
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
                                let raw = rec.value_at(param.file_offset);
                                json!({
                                    "name": param.name,
                                    "label": param.label(),
                                    "value": raw,
                                    "offset": param.file_offset,
                                    "minimum": param.minimum(),
                                    "maximum": param.maximum(),
                                    "control": param.control().as_str(),
                                    "unit": param.unit,
                                    "midi_cc": param.midi_cc,
                                    "confirmed": param.is_confirmed(),
                                    // Wartość fizyczna (dB/Hz), gdy znany wzór (W3 uzupełnienie).
                                    "display": param.display_value(raw),
                                    // Enum (np. POSITION): etykieta bieżącego stanu + wartość
                                    // następnego stanu (do przełącznika/toggle).
                                    "enum_label": param.enum_label(raw),
                                    "enum_next": param.enum_next(raw),
                                    "enum_next_label": param.enum_label(param.enum_next(raw)),
                                    "enum_labels": param.enum_labels(),
                                    "enum_values": param.enum_values(),
                                    "enum_index": param.enum_index(raw),
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
            // Metadane opisowe — kto jest autorem i na jakich warunkach patch może być użyty.
            "meta": meta_json(&item),
            "tags": item.tags.iter().collect::<Vec<_>>(),
            "collections": item.groups.iter().collect::<Vec<_>>(),
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

    /// Surowe bajty rekordu pogrupowane logicznie (inspektor binarny). Region
    /// próbek IR (0xD2.., 8 KB) jest streszczony — zwracamy podgląd, nie 8 KB.
    fn get_raw(&self, id: &PatchId) -> Result<Value, ExecError> {
        const IR_PREVIEW: usize = 32;
        let item = self.get_patch(id)?;
        let rec = self.record(&item)?;
        let data = rec.data();
        let p = self.profile;

        // Bajt jako grupa: etykieta, offset, kind, bajty (+ opcjonalny podgląd).
        let byte_at = |o: usize| data.get(o).copied().unwrap_or(0) as i64;
        let slice = |start: usize, len: usize| -> Vec<i64> {
            (start..(start + len).min(data.len()))
                .map(|o| data[o] as i64)
                .collect()
        };
        let mut groups: Vec<Value> = Vec::new();
        let mut push = |label: String, offset: usize, kind: &str, bytes: Vec<i64>| {
            groups.push(json!({
                "label": label, "offset": offset, "kind": kind,
                "length": bytes.len(), "bytes": bytes,
            }));
        };

        // Nagłówek: indeks slotu (0x00..0x04).
        push("slot.index".into(), 0, "header", slice(0, 4));

        // Bloki łańcucha: selektor (model_id + bypass) i bajty parametrów.
        for block in &p.blocks {
            let sel = byte_at(block.selector_offset);
            let model_id = sel & 0x3F;
            let bypass = (sel & 0x40) != 0;
            let name = self
                .catalog
                .model(&block.id, model_id)
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| format!("model {model_id}"));
            push(
                format!(
                    "{} = {name}{}",
                    block.id,
                    if bypass { " (bypass)" } else { "" }
                ),
                block.selector_offset,
                "selector",
                vec![sel],
            );
            for (i, &off) in block.parameter_offsets.iter().enumerate() {
                push(
                    format!("{}.param[{i}]", block.id),
                    off,
                    "param",
                    vec![byte_at(off)],
                );
            }
        }

        // BPM (dwa bajty 7-bitowe).
        push(
            "bpm".into(),
            p.bpm.msb_offset,
            "meta",
            vec![byte_at(p.bpm.msb_offset), byte_at(p.bpm.lsb_offset)],
        );
        // Nazwa presetu (zakres) — także zdekodowana w etykiecie.
        push(
            format!("nazwa = \"{}\"", rec.name()),
            p.patch_name.offset,
            "name",
            slice(p.patch_name.offset, p.patch_name.length),
        );
        // Pola nazwane (sortowane po offsecie dla stabilnej kolejności).
        let mut nf: Vec<_> = p.named_fields.iter().collect();
        nf.sort_by_key(|(_, f)| f.offset);
        for (k, f) in nf {
            push(
                format!("pole: {k}"),
                f.offset,
                "meta",
                vec![byte_at(f.offset)],
            );
        }

        // Region IR: marker+nazwa, RIFF, próbki (streszczone).
        let ir_marker = 0x82usize;
        let ir_riff = 0xA6usize;
        let ir_samples = 0xD2usize;
        if data.len() > ir_marker {
            push(
                "ir.marker+name".into(),
                ir_marker,
                "ir",
                slice(ir_marker, ir_riff - ir_marker),
            );
            push(
                "ir.riff_header".into(),
                ir_riff,
                "ir",
                slice(ir_riff, ir_samples - ir_riff),
            );
            let sample_len = data.len().saturating_sub(ir_samples);
            groups.push(json!({
                "label": format!("ir.samples ({sample_len} B, podgląd {IR_PREVIEW})"),
                "offset": ir_samples, "kind": "ir",
                "length": sample_len,
                "bytes": slice(ir_samples, IR_PREVIEW),
                "truncated": true,
            }));
        }

        Ok(json!({ "size": data.len(), "groups": groups }))
    }

    fn get_diff(&self, id: &PatchId) -> Result<Value, ExecError> {
        let item = self.get_patch(id)?;
        let rec = self.record(&item)?;
        // Baza różnicy to bajty z chwili utworzenia/importu (parytet v1: „zmiany
        // względem importowanego oryginału"). Gdy brak baseline (stare wpisy) →
        // porównaj z bieżącym blobem, czyli pokaż BRAK zmian (bez szumu 0→wartość).
        let baseline_bytes = if item.baseline_blob.is_empty() {
            item.blob.clone()
        } else {
            item.baseline_blob.clone()
        };
        let baseline = PatchRecord::new(baseline_bytes, self.profile)?;
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

    /// Edycja metadanych patcha — **poza blobem**. Bajty patcha (i tym samym
    /// fingerprinty) pozostają nietknięte, więc nie odświeżamy hashy ani nie
    /// journalujemy do WAL: `revert_last` przywraca BAJTY, a tu żaden bajt brzmienia
    /// się nie zmienia. Rewizja jest sprawdzana i podbijana jak przy każdej mutacji.
    fn mutate_meta<F>(&mut self, target: &TargetRef, edit: F) -> Result<Value, ExecError>
    where
        F: FnOnce(&mut LibraryPatch),
    {
        let (patch_id, revision) = library_target(target)?;
        let mut item = self.get_patch(&patch_id)?;
        if item.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: item.revision,
            });
        }
        edit(&mut item);
        item.updated_at = self.now_ms;
        let new_rev = self.store.update(item, revision)?;
        Ok(json!({"success": true, "revision": new_rev}))
    }

    fn set_patch_meta(
        &mut self,
        target: &TargetRef,
        meta: &mg101_commands::MetaPatch,
    ) -> Result<Value, ExecError> {
        if meta.is_empty() {
            return Err(ExecError::Unsupported("brak pól do zmiany".into()));
        }
        let meta = meta.clone();
        self.mutate_meta(target, move |item| {
            // Pusty string = wyczyść pole; brak pola (None) = nie ruszaj.
            fn assign(dst: &mut Option<String>, src: Option<String>) {
                if let Some(v) = src {
                    *dst = if v.trim().is_empty() {
                        None
                    } else {
                        Some(v.trim().to_string())
                    };
                }
            }
            assign(&mut item.meta.author, meta.author);
            assign(&mut item.meta.source, meta.source);
            assign(&mut item.meta.source_url, meta.source_url);
            assign(&mut item.meta.license, meta.license);
            assign(&mut item.meta.notes, meta.notes);
            if let Some(r) = meta.rating {
                item.meta.set_rating(r.clamp(0, i64::from(u8::MAX)) as u8);
            }
            if let Some(f) = meta.favorite {
                item.meta.favorite = f;
            }
        })
    }

    fn change_tag(&mut self, target: &TargetRef, tag: &str, add: bool) -> Result<Value, ExecError> {
        let tag = tag.trim().to_string();
        if tag.is_empty() {
            return Err(ExecError::Unsupported("pusty tag".into()));
        }
        self.mutate_meta(target, move |item| {
            if add {
                item.tags.insert(tag);
            } else {
                item.tags.remove(&tag);
            }
        })
    }

    fn list_collections(&self) -> Value {
        let mut groups: Vec<Value> = self
            .store
            .groups()
            .into_iter()
            .map(|g| {
                json!({
                    "id": g.id,
                    "name": g.name,
                    "count": g.members.len(),
                    "members": g.members,
                })
            })
            .collect();
        groups.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or_default().to_lowercase())
        });
        json!({ "collections": groups })
    }

    fn create_collection(&mut self, name: &str) -> Result<Value, ExecError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ExecError::Unsupported("pusta nazwa kolekcji".into()));
        }
        // ID stabilne i czytelne (slug), unikalne przez licznik przy kolizji.
        let base = slug(name);
        let existing: std::collections::BTreeSet<String> =
            self.store.groups().into_iter().map(|g| g.id).collect();
        let mut id = base.clone();
        let mut n = 2;
        while existing.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.store.create_group(mg101_library::Group {
            id: id.clone(),
            name: name.to_string(),
            members: Vec::new(),
        })?;
        Ok(json!({"success": true, "id": id, "name": name}))
    }

    fn delete_collection(&mut self, id: &str) -> Result<Value, ExecError> {
        // Store czyści też przynależność w patchach — same patche zostają.
        self.store.delete_group(&id.to_string())?;
        Ok(json!({"success": true}))
    }

    fn change_collection(
        &mut self,
        target: &TargetRef,
        collection: &str,
        add: bool,
    ) -> Result<Value, ExecError> {
        let (patch_id, revision) = library_target(target)?;
        let item = self.get_patch(&patch_id)?;
        if item.revision != revision {
            return Err(ExecError::Conflict {
                current_revision: item.revision,
            });
        }
        let gid = collection.to_string();
        // Store utrzymuje OBIE strony relacji (members grupy + groups patcha) —
        // dlatego nie edytujemy `item.groups` ręcznie, bo rozjechałaby się grupa.
        if add {
            self.store.add_to_group(&gid, &patch_id)?;
        } else {
            self.store.remove_from_group(&gid, &patch_id)?;
        }
        let rev = self
            .store
            .get(&patch_id)
            .map(|p| p.revision)
            .unwrap_or(revision);
        Ok(json!({"success": true, "revision": rev}))
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
        copy.baseline_blob = copy.blob.clone(); // baza = stan z chwili duplikacji
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
        for chunk in data.chunks(rs) {
            // Walidacja: rekord musi się dekodować profilem (bajty święte zachowane).
            PatchRecord::new(chunk.to_vec(), self.profile)?;
            let blob = chunk.to_vec();
            // ID adresowane treścią: deterministyczne między sesjami (SqliteStore jest
            // trwały, a licznik w pamięci zerował się przy restarcie → kolizje "import-1-0").
            // Re-import tych samych bajtów jest teraz idempotentny zamiast błędu.
            let exact_hash = mg101_library::exact_hash(&blob);
            let id = format!("import-{}", &exact_hash[..exact_hash.len().min(16)]);
            if self.store.get(&id).is_some() {
                ids.push(Value::String(id));
                continue;
            }
            let mut patch = LibraryPatch {
                id: id.clone(),
                name: String::new(),
                content_hash: String::new(),
                exact_hash,
                baseline_blob: blob.clone(),
                blob,
                origin: PatchOrigin::ImportedFile,
                device_id: self.profile.id.clone(),
                firmware: None,
                codec_version: "1".into(),
                tags: Default::default(),
                groups: Default::default(),
                meta: Default::default(),
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
/// Metadane patcha jako JSON (jeden kształt dla `list_patches` i `get_patch`).
fn meta_json(item: &LibraryPatch) -> Value {
    json!({
        "author": item.meta.author,
        "source": item.meta.source,
        "sourceURL": item.meta.source_url,
        "license": item.meta.license,
        "notes": item.meta.notes,
        "rating": item.meta.rating,
        "favorite": item.meta.favorite,
    })
}

/// Zamienia nazwę na stabilny, czytelny identyfikator (ASCII, bez spacji).
/// Pusty wynik (np. sama interpunkcja) zastępujemy `collection`, by ID nigdy nie było puste.
fn slug(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    // Zwiń wielokrotne myślniki.
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                out.push(c);
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    if out.is_empty() {
        "collection".to_string()
    } else {
        out
    }
}

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

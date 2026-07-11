//! ViewModel desktopu — warstwa adaptacyjna między [`Studio`] a UI (Slint).
//!
//! Cały stan i mutacje przechodzą przez jedną magistralę [`Studio::execute`]
//! (te same [`Command`], co agent i MCP — ADR-0002), a wyniki JSON są tu
//! parsowane na płaskie struktury UI. Warstwa jest **bez zależności od Slint**
//! (i platformy) — logika listy/edytora/inspektora/zakładek jest testowalna bez
//! okna. Powłoka Slint (`ui`/`main`) tylko renderuje te struktury i wywołuje
//! metody.

use mg101_commands::{Command, TargetRef};
use mg101_library::{LibraryStore, PatchOrigin};
use mg101_studio::{ExecError, Studio};
use serde_json::Value;

use crate::i18n::{tr, Lang};

/// Zakładka biblioteki/urządzenia (nowość v2 — HLD §5). `Library` to skład
/// aplikacji; `User`/`Factory` to widoki banków urządzenia (puste bez połączenia).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryTab {
    User,
    Factory,
    Library,
}

/// Wiersz listy patchy (Biblioteka).
#[derive(Debug, Clone, PartialEq)]
pub struct PatchRow {
    pub patch_id: String,
    pub slot: usize,
    pub name: String,
    pub origin: String,
    pub revision: i64,
    pub ir_present: bool,
}

/// Wiersz slotu urządzenia (zakładki User/Factory).
#[derive(Debug, Clone, PartialEq)]
pub struct SlotRow {
    pub index: u16,
    pub name: String,
    pub occupied: bool,
    pub writable: bool,
}

/// Parametr aktywnego modelu bloku.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamRow {
    pub name: String,
    pub value: i64,
    pub minimum: i64,
    pub maximum: i64,
}

/// Blok w łańcuchu efektów (widok edytora).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockRow {
    pub block: String,
    pub model_id: i64,
    pub model_name: String,
    pub bypassed: bool,
    pub parameters: Vec<ParamRow>,
}

/// Szczegóły wybranego patcha (edytor + nagłówek).
#[derive(Debug, Clone, PartialEq)]
pub struct PatchDetail {
    pub patch_id: String,
    pub name: String,
    pub bpm: i64,
    pub revision: i64,
    pub ir_present: bool,
    pub blocks: Vec<BlockRow>,
}

/// Pojedyncza różnica bajtowa (inspektor Changes).
#[derive(Debug, Clone, PartialEq)]
pub struct ByteChange {
    pub offset: usize,
    pub before: i64,
    pub after: i64,
}

/// ViewModel: posiada [`Studio`] i bieżący język; utrzymuje migawki banków
/// urządzenia (wstrzykiwane z warstwy device-link po połączeniu).
pub struct ViewModel<S: LibraryStore> {
    studio: Studio<'static, S>,
    lang: Lang,
    tab: LibraryTab,
    user_bank: Vec<SlotRow>,
    factory_bank: Vec<SlotRow>,
    last_error: Option<String>,
}

impl<S: LibraryStore> ViewModel<S> {
    pub fn new(studio: Studio<'static, S>, lang: Lang) -> Self {
        Self {
            studio,
            lang,
            tab: LibraryTab::Library,
            user_bank: Vec::new(),
            factory_bank: Vec::new(),
            last_error: None,
        }
    }

    pub fn lang(&self) -> Lang {
        self.lang
    }

    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
    }

    pub fn tab(&self) -> LibraryTab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: LibraryTab) {
        self.tab = tab;
    }

    /// Ostatni komunikat błędu (już zlokalizowany), jeśli był.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn clear_error(&mut self) {
        self.last_error = None;
    }

    /// Wstrzykuje migawkę banków urządzenia (po połączeniu/dump — E2/E5).
    pub fn set_device_banks(&mut self, user: Vec<SlotRow>, factory: Vec<SlotRow>) {
        self.user_bank = user;
        self.factory_bank = factory;
    }

    /// Etykieta zlokalizowana (delegat do i18n — jedno miejsce dla UI).
    pub fn label(&self, key: &str) -> &'static str {
        tr(self.lang, key)
    }

    // --- Odczyt ---

    fn exec(&mut self, command: &Command) -> Option<Value> {
        match self.studio.execute(command) {
            Ok(v) => Some(v),
            Err(e) => {
                self.last_error = Some(self.localize_error(&e));
                None
            }
        }
    }

    /// Wiersze aktywnej zakładki. Library → skład aplikacji; User/Factory →
    /// migawki banków urządzenia (puste bez połączenia).
    pub fn rows(&mut self) -> Vec<PatchRow> {
        if self.tab != LibraryTab::Library {
            return Vec::new();
        }
        self.library_rows()
    }

    /// Sloty aktywnej zakładki urządzenia (puste dla zakładki Library).
    pub fn slot_rows(&self) -> &[SlotRow] {
        match self.tab {
            LibraryTab::User => &self.user_bank,
            LibraryTab::Factory => &self.factory_bank,
            LibraryTab::Library => &[],
        }
    }

    /// Pełna lista patchy biblioteki (zakładka Library).
    pub fn library_rows(&mut self) -> Vec<PatchRow> {
        let v = match self.exec(&Command::ListPatches) {
            Some(v) => v,
            None => return Vec::new(),
        };
        v.as_array()
            .map(|arr| arr.iter().map(row_from_json).collect())
            .unwrap_or_default()
    }

    /// Wiersze biblioteki tylko patchy danego pochodzenia (filtr pomocniczy UI).
    pub fn library_rows_with_origin(&mut self, origin: PatchOrigin) -> Vec<PatchRow> {
        let want = origin_str(origin);
        self.library_rows()
            .into_iter()
            .filter(|r| r.origin == want)
            .collect()
    }

    pub fn selected_id(&mut self) -> Option<String> {
        let v = self.exec(&Command::GetSelection)?;
        v.get("selectedPatchID")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Zaznacza patch (przełącza widok edytora).
    pub fn select(&mut self, patch_id: &str) -> bool {
        self.exec(&Command::SelectPatch {
            patch_id: patch_id.to_owned(),
        })
        .is_some()
    }

    /// Szczegóły patcha (edytor). `None` przy błędzie/braku.
    pub fn detail(&mut self, patch_id: &str) -> Option<PatchDetail> {
        let v = self.exec(&Command::GetPatch {
            patch_id: patch_id.to_owned(),
        })?;
        Some(detail_from_json(&v))
    }

    /// Szczegóły aktualnie zaznaczonego patcha.
    pub fn selected_detail(&mut self) -> Option<PatchDetail> {
        let id = self.selected_id()?;
        self.detail(&id)
    }

    fn current_revision(&mut self, patch_id: &str) -> Option<i64> {
        let v = self.exec(&Command::GetPatch {
            patch_id: patch_id.to_owned(),
        })?;
        v.get("revision").and_then(Value::as_i64)
    }

    /// Różnice bajtowe patcha względem oryginału (inspektor Changes).
    pub fn changes(&mut self, patch_id: &str) -> Vec<ByteChange> {
        let v = match self.exec(&Command::GetDiff {
            patch_id: patch_id.to_owned(),
        }) {
            Some(v) => v,
            None => return Vec::new(),
        };
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .map(|d| ByteChange {
                        offset: d.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
                        before: d.get("before").and_then(Value::as_i64).unwrap_or(0),
                        after: d.get("after").and_then(Value::as_i64).unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // --- Mutacje (przez magistralę komend, cel biblioteczny z rewizją) ---

    fn library_target(&mut self, patch_id: &str) -> Option<TargetRef> {
        let revision = self.current_revision(patch_id)?;
        Some(TargetRef::Library {
            patch_id: patch_id.to_owned(),
            expected_revision: revision,
        })
    }

    /// Zmienia parametr aktywnego modelu bloku.
    pub fn set_parameter(
        &mut self,
        patch_id: &str,
        block: &str,
        parameter: &str,
        value: i64,
    ) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::SetParameter {
            target,
            block: block.to_owned(),
            parameter: parameter.to_owned(),
            value,
        })
        .is_some()
    }

    /// Zmienia aktywny model bloku wraz z wartościami i bypassem.
    pub fn set_model(
        &mut self,
        patch_id: &str,
        block: &str,
        model: i64,
        values: Vec<i64>,
        bypassed: bool,
    ) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::SetModel {
            target,
            block: block.to_owned(),
            model,
            values,
            bypassed,
        })
        .is_some()
    }

    /// Włącza/wyłącza bypass bloku.
    pub fn set_bypass(&mut self, patch_id: &str, block: &str, bypassed: bool) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::SetBypass {
            target,
            block: block.to_owned(),
            bypassed,
        })
        .is_some()
    }

    /// Zmienia nazwę patcha.
    pub fn rename(&mut self, patch_id: &str, name: &str) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::SetName {
            target,
            name: name.to_owned(),
        })
        .is_some()
    }

    /// Zmienia BPM patcha.
    pub fn set_bpm(&mut self, patch_id: &str, bpm: i64) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::SetBpm { target, bpm }).is_some()
    }

    /// Duplikuje patch; zwraca ID nowej kopii.
    pub fn duplicate(&mut self, patch_id: &str) -> Option<String> {
        let target = self.library_target(patch_id)?;
        let v = self.exec(&Command::DuplicatePatch { target })?;
        v.get("newPatchID")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Usuwa (miękko) patch do poczekalni sesji.
    pub fn delete(&mut self, patch_id: &str) -> bool {
        let Some(target) = self.library_target(patch_id) else {
            return false;
        };
        self.exec(&Command::DeletePatch { target }).is_some()
    }

    /// Cofa ostatnią operację agenta/UI w sesji (wymaga dziennika WAL).
    pub fn revert_last(&mut self) -> bool {
        self.exec(&Command::RevertLastAgentAction).is_some()
    }

    fn localize_error(&self, e: &ExecError) -> String {
        // Prefiks zlokalizowany + treść techniczna (spójne z alertem v1).
        format!("{}: {e}", tr(self.lang, "error.title"))
    }
}

fn origin_str(origin: PatchOrigin) -> &'static str {
    // Zgodne z `format!("{:?}", origin)` w Studio::list_patches.
    match origin {
        PatchOrigin::ImportedFile => "ImportedFile",
        PatchOrigin::PulledFromDevice => "PulledFromDevice",
        PatchOrigin::Created => "Created",
        PatchOrigin::Shared => "Shared",
    }
}

fn row_from_json(v: &Value) -> PatchRow {
    PatchRow {
        patch_id: v
            .get("patchID")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        slot: v.get("slot").and_then(Value::as_u64).unwrap_or(0) as usize,
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        origin: v
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        revision: v.get("revision").and_then(Value::as_i64).unwrap_or(0),
        ir_present: v
            .get("ir")
            .and_then(|ir| ir.get("present"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn detail_from_json(v: &Value) -> PatchDetail {
    let blocks = v
        .get("blocks")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().map(block_from_json).collect())
        .unwrap_or_default();
    PatchDetail {
        patch_id: v
            .get("patchID")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        bpm: v.get("bpm").and_then(Value::as_i64).unwrap_or(0),
        revision: v.get("revision").and_then(Value::as_i64).unwrap_or(0),
        ir_present: v
            .get("ir")
            .and_then(|ir| ir.get("present"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        blocks,
    }
}

fn block_from_json(v: &Value) -> BlockRow {
    let parameters = v
        .get("parameters")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|p| ParamRow {
                    name: p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    value: p.get("value").and_then(Value::as_i64).unwrap_or(0),
                    minimum: p.get("minimum").and_then(Value::as_i64).unwrap_or(0),
                    maximum: p.get("maximum").and_then(Value::as_i64).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    BlockRow {
        block: v
            .get("block")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        model_id: v.get("current_model").and_then(Value::as_i64).unwrap_or(0),
        model_name: v
            .get("current_model_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        bypassed: v.get("bypassed").and_then(Value::as_bool).unwrap_or(false),
        parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mg101_core::wal::{JournalStore, TransactionEntry, WalError};
    use mg101_core::{DeviceProfile, EffectCatalog};
    use mg101_library::{LibraryPatch, LibraryStore, MemoryStore};
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::rc::Rc;

    #[derive(Default)]
    struct MemJournal {
        entries: RefCell<Vec<TransactionEntry>>,
    }
    struct SharedJournal(Rc<MemJournal>);
    impl JournalStore for SharedJournal {
        fn append(&self, entry: &TransactionEntry) -> Result<(), WalError> {
            self.0.entries.borrow_mut().push(entry.clone());
            Ok(())
        }
        fn load(&self) -> Result<Vec<TransactionEntry>, WalError> {
            Ok(self.0.entries.borrow().clone())
        }
        fn rewrite(&self, entries: &[TransactionEntry]) -> Result<(), WalError> {
            *self.0.entries.borrow_mut() = entries.to_vec();
            Ok(())
        }
    }

    fn patch(id: &str, record_size: usize, origin: PatchOrigin) -> LibraryPatch {
        LibraryPatch {
            id: id.into(),
            name: id.into(),
            blob: vec![0u8; record_size],
            origin,
            device_id: "nux-mg101".into(),
            firmware: None,
            codec_version: "1".into(),
            content_hash: "c".into(),
            exact_hash: "e".into(),
            tags: BTreeSet::new(),
            groups: BTreeSet::new(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
        }
    }

    fn vm() -> ViewModel<MemoryStore> {
        let (profile, catalog) = mg101_pack_nux_mg101::load().unwrap();
        let profile: &'static DeviceProfile = Box::leak(Box::new(profile));
        let catalog: &'static EffectCatalog = Box::leak(Box::new(catalog));
        let mut store = MemoryStore::new();
        store
            .add(patch("p1", profile.record_size, PatchOrigin::Created))
            .unwrap();
        store
            .add(patch("p2", profile.record_size, PatchOrigin::ImportedFile))
            .unwrap();
        let journal = Box::new(SharedJournal(Rc::new(MemJournal::default())));
        let studio = Studio::new(store, profile, catalog, 100).with_journal(journal);
        ViewModel::new(studio, Lang::En)
    }

    #[test]
    fn library_tab_lists_all_patches() {
        let mut vm = vm();
        vm.set_tab(LibraryTab::Library);
        let rows = vm.rows();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.patch_id == "p1"));
    }

    #[test]
    fn device_tabs_are_empty_without_connection_then_populated() {
        let mut vm = vm();
        vm.set_tab(LibraryTab::User);
        assert!(vm.rows().is_empty()); // Library-only rows
        assert!(vm.slot_rows().is_empty());

        vm.set_device_banks(
            vec![SlotRow {
                index: 0,
                name: "A".into(),
                occupied: true,
                writable: true,
            }],
            vec![SlotRow {
                index: 0,
                name: "F".into(),
                occupied: true,
                writable: false,
            }],
        );
        vm.set_tab(LibraryTab::User);
        assert_eq!(vm.slot_rows().len(), 1);
        assert!(vm.slot_rows()[0].writable);
        vm.set_tab(LibraryTab::Factory);
        assert!(
            !vm.slot_rows()[0].writable,
            "bank fabryczny tylko do odczytu"
        );
    }

    #[test]
    fn origin_filter_splits_user_vs_imported() {
        let mut vm = vm();
        assert_eq!(vm.library_rows_with_origin(PatchOrigin::Created).len(), 1);
        assert_eq!(
            vm.library_rows_with_origin(PatchOrigin::ImportedFile).len(),
            1
        );
    }

    #[test]
    fn select_then_detail_returns_blocks() {
        let mut vm = vm();
        assert!(vm.select("p1"));
        assert_eq!(vm.selected_id().as_deref(), Some("p1"));
        let d = vm.selected_detail().unwrap();
        assert_eq!(d.patch_id, "p1");
        assert!(!d.blocks.is_empty());
    }

    #[test]
    fn edit_bpm_routes_through_command_bus_and_bumps_revision() {
        let mut vm = vm();
        let before = vm.detail("p1").unwrap().revision;
        assert!(vm.set_bpm("p1", 123));
        let d = vm.detail("p1").unwrap();
        assert_eq!(d.bpm, 123);
        assert_eq!(
            d.revision,
            before + 1,
            "rewizja podbita (optimistic concurrency)"
        );
    }

    #[test]
    fn edit_parameter_and_bypass_persist() {
        let mut vm = vm();
        let d = vm.detail("p1").unwrap();
        let blk = &d.blocks[0];
        assert!(vm.set_bypass("p1", &blk.block, true));
        assert!(vm.detail("p1").unwrap().blocks[0].bypassed);

        if let Some(param) = d
            .blocks
            .iter()
            .flat_map(|b| {
                b.parameters
                    .iter()
                    .map(move |p| (b.block.clone(), p.clone()))
            })
            .next()
        {
            let (block, p) = param;
            let target = (p.minimum + p.maximum) / 2;
            assert!(vm.set_parameter("p1", &block, &p.name, target));
        }
    }

    #[test]
    fn duplicate_creates_new_patch_and_delete_removes() {
        let mut vm = vm();
        let new_id = vm.duplicate("p1").unwrap();
        assert_ne!(new_id, "p1");
        assert_eq!(vm.library_rows().len(), 3);
        assert!(vm.delete(&new_id));
        assert_eq!(vm.library_rows().len(), 2);
    }

    #[test]
    fn changes_reports_byte_diffs_after_edit() {
        let mut vm = vm();
        assert!(vm.set_bpm("p1", 140));
        let changes = vm.changes("p1");
        assert!(
            !changes.is_empty(),
            "edycja BPM powinna dać różnice bajtowe"
        );
    }

    #[test]
    fn error_is_localized_and_surfaced() {
        let mut vm = vm();
        // Nieistniejący patch → błąd wypełnia last_error zlokalizowanym prefiksem.
        assert!(vm.detail("nope").is_none());
        let err = vm.last_error().unwrap();
        assert!(err.starts_with("Operation failed"), "EN prefiks: {err}");
        vm.clear_error();
        assert!(vm.last_error().is_none());

        vm.set_lang(Lang::Pl);
        assert!(vm.detail("nope").is_none());
        assert!(vm
            .last_error()
            .unwrap()
            .starts_with("Operacja nie powiodła się"));
    }

    #[test]
    fn revert_last_undoes_edit() {
        let mut vm = vm();
        assert!(vm.set_bpm("p1", 200));
        assert_eq!(vm.detail("p1").unwrap().bpm, 200);
        assert!(vm.revert_last());
        assert_ne!(
            vm.detail("p1").unwrap().bpm,
            200,
            "revert cofnął zmianę BPM"
        );
    }
}

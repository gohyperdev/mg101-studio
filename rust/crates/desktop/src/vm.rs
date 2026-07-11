//! ViewModel desktopu — warstwa adaptacyjna między [`Studio`] a UI (Slint).
//!
//! Cały stan i mutacje przechodzą przez jedną magistralę [`Studio::execute`]
//! (te same [`Command`], co agent i MCP — ADR-0002), a wyniki JSON są tu
//! parsowane na płaskie struktury UI. Warstwa jest **bez zależności od Slint**
//! (i platformy) — logika listy/edytora/inspektora/zakładek jest testowalna bez
//! okna. Powłoka Slint (`ui`/`main`) tylko renderuje te struktury i wywołuje
//! metody.

use mg101_agent_core::{
    default_pricing, pricing_for, AgentConfig, ChatMessage, Provider, Role, Usage,
};
use mg101_commands::{Command, TargetRef};
use mg101_library::{LibraryStore, PatchOrigin};
use mg101_studio::{ExecError, Studio};
use serde_json::Value;

use crate::i18n::{tr, Lang};

/// Wiersz rozmowy agenta do wyświetlenia (rola + tekst; wywołania narzędzi
/// pokazywane jako skrót).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    pub role: String,
    pub text: String,
}

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

/// Opcja modelu w pickerze bloku.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOption {
    pub id: i64,
    pub name: String,
    /// Domyślne wartości parametrów (minima) — używane przy przełączeniu modelu.
    pub defaults: Vec<i64>,
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
    agent_config: AgentConfig,
    chat: Vec<ChatMessage>,
    session_usage: Usage,
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
            agent_config: default_agent_config(),
            chat: Vec::new(),
            session_usage: Usage::default(),
        }
    }

    // --- Agent (konfiguracja, rozmowa, rozliczenie sesji) ---

    pub fn agent_config(&self) -> &AgentConfig {
        &self.agent_config
    }

    pub fn set_agent_config(&mut self, config: AgentConfig) {
        self.agent_config = config.normalized();
    }

    /// Czy konfiguracja pozwala wywołać model (endpoint+model+klucz wg dostawcy).
    pub fn is_agent_configured(&self) -> bool {
        self.agent_config.is_configured()
    }

    /// Pełna historia rozmowy (kopia dla wątku agenta).
    pub fn chat_history(&self) -> Vec<ChatMessage> {
        self.chat.clone()
    }

    /// Dokłada wiadomość użytkownika do rozmowy.
    pub fn push_user_message(&mut self, text: &str) {
        self.chat.push(ChatMessage::user(text.to_owned()));
    }

    /// Zastępuje historię wynikiem przebiegu agenta i dolicza zużycie tokenów.
    pub fn apply_run_outcome(&mut self, history: Vec<ChatMessage>, usage: Usage) {
        self.chat = history;
        self.session_usage.add(usage);
    }

    /// Wiersze rozmowy do wyświetlenia (rola + tekst; wywołania narzędzi jako skrót).
    pub fn chat_rows(&self) -> Vec<ChatRow> {
        let mut rows = Vec::new();
        for m in &self.chat {
            let role = match m.role {
                Role::User if m.tool_results.is_empty() => "user",
                Role::User => "tool",
                Role::Assistant => "assistant",
            };
            if role == "tool" {
                // Wyniki narzędzi — zwięzły ślad, nie zalewać rozmowy.
                rows.push(ChatRow {
                    role: "tool".into(),
                    text: format!("[{} wynik(ów) narzędzi]", m.tool_results.len()),
                });
                continue;
            }
            let mut text = m.content.clone();
            for tc in &m.tool_calls {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("→ narzędzie: {}", tc.name));
            }
            if !text.is_empty() {
                rows.push(ChatRow {
                    role: role.into(),
                    text,
                });
            }
        }
        rows
    }

    pub fn session_usage(&self) -> Usage {
        self.session_usage
    }

    /// Koszt sesji w USD wg cennika modelu (0 dla nieznanego modelu).
    pub fn session_cost_usd(&self) -> f64 {
        let table = default_pricing();
        pricing_for(&self.agent_config.model, &table)
            .map(|p| p.cost_usd(self.session_usage))
            .unwrap_or(0.0)
    }

    /// Wykonuje komendę narzędzia agenta na składzie (dla egzekutora kanałowego
    /// na wątku UI). Zwraca JSON lub komunikat błędu (kształt dla modelu).
    pub fn execute_tool(&mut self, command: &Command) -> Result<Value, String> {
        self.studio.execute(command).map_err(|e| e.to_string())
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

    /// Ustawia zlokalizowany komunikat błędu z zewnątrz (np. błąd wątku agenta).
    pub fn report_error(&mut self, detail: String) {
        self.last_error = Some(format!("{}: {detail}", tr(self.lang, "error.title")));
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

    /// Modele dostępne dla bloku (picker) wraz z domyślnymi wartościami params.
    pub fn models(&mut self, block: &str) -> Vec<ModelOption> {
        let v = match self.exec(&Command::ListModels {
            block: block.to_owned(),
        }) {
            Some(v) => v,
            None => return Vec::new(),
        };
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .map(|m| ModelOption {
                        id: m.get("modelID").and_then(Value::as_i64).unwrap_or(0),
                        name: m
                            .get("displayName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        defaults: m
                            .get("parameters")
                            .and_then(Value::as_array)
                            .map(|ps| {
                                ps.iter()
                                    .map(|p| p.get("minimum").and_then(Value::as_i64).unwrap_or(0))
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Przełącza aktywny model bloku na `model_id`, ustawiając jego parametry na
    /// wartości domyślne (minima). Parytet pickera modelu z v1 `BlockEditorView`.
    pub fn switch_model(
        &mut self,
        patch_id: &str,
        expected_revision: i64,
        block: &str,
        model_id: i64,
    ) -> bool {
        let Some(opt) = self.models(block).into_iter().find(|m| m.id == model_id) else {
            self.last_error = Some(format!(
                "{}: nieznany model {block}.{model_id}",
                tr(self.lang, "error.title")
            ));
            return false;
        };
        self.set_model(
            patch_id,
            expected_revision,
            block,
            model_id,
            opt.defaults,
            false,
        )
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
    //
    // KRYTYCZNE (review E7/K1): `expected_revision` musi pochodzić z rewizji,
    // którą WIDZIAŁ użytkownik (z `detail`/wiersza), a NIE być doczytywany tuż
    // przed zapisem — inaczej optimistic concurrency jest fikcją (TOCTOU: przy
    // współdzielonym składzie UI po cichu nadpisałoby cudzą zmianę). Dlatego
    // każda mutacja przyjmuje jawnie `expected_revision`.

    fn library_target(patch_id: &str, expected_revision: i64) -> TargetRef {
        TargetRef::Library {
            patch_id: patch_id.to_owned(),
            expected_revision,
        }
    }

    /// Zmienia parametr aktywnego modelu bloku.
    pub fn set_parameter(
        &mut self,
        patch_id: &str,
        expected_revision: i64,
        block: &str,
        parameter: &str,
        value: i64,
    ) -> bool {
        self.exec(&Command::SetParameter {
            target: Self::library_target(patch_id, expected_revision),
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
        expected_revision: i64,
        block: &str,
        model: i64,
        values: Vec<i64>,
        bypassed: bool,
    ) -> bool {
        self.exec(&Command::SetModel {
            target: Self::library_target(patch_id, expected_revision),
            block: block.to_owned(),
            model,
            values,
            bypassed,
        })
        .is_some()
    }

    /// Włącza/wyłącza bypass bloku.
    pub fn set_bypass(
        &mut self,
        patch_id: &str,
        expected_revision: i64,
        block: &str,
        bypassed: bool,
    ) -> bool {
        self.exec(&Command::SetBypass {
            target: Self::library_target(patch_id, expected_revision),
            block: block.to_owned(),
            bypassed,
        })
        .is_some()
    }

    /// Zmienia nazwę patcha.
    pub fn rename(&mut self, patch_id: &str, expected_revision: i64, name: &str) -> bool {
        self.exec(&Command::SetName {
            target: Self::library_target(patch_id, expected_revision),
            name: name.to_owned(),
        })
        .is_some()
    }

    /// Zmienia BPM patcha.
    pub fn set_bpm(&mut self, patch_id: &str, expected_revision: i64, bpm: i64) -> bool {
        self.exec(&Command::SetBpm {
            target: Self::library_target(patch_id, expected_revision),
            bpm,
        })
        .is_some()
    }

    /// Duplikuje patch; zwraca ID nowej kopii.
    pub fn duplicate(&mut self, patch_id: &str, expected_revision: i64) -> Option<String> {
        let v = self.exec(&Command::DuplicatePatch {
            target: Self::library_target(patch_id, expected_revision),
        })?;
        v.get("newPatchID")
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// Usuwa (miękko) patch do poczekalni sesji.
    pub fn delete(&mut self, patch_id: &str, expected_revision: i64) -> bool {
        self.exec(&Command::DeletePatch {
            target: Self::library_target(patch_id, expected_revision),
        })
        .is_some()
    }

    /// Cofa ostatnią operację agenta/UI w sesji (wymaga dziennika WAL).
    pub fn revert_last(&mut self) -> bool {
        self.exec(&Command::RevertLastAgentAction).is_some()
    }

    /// Eksportuje patch do pliku `.mg101patch` pod wskazaną ścieżką (offline —
    /// nie wymaga urządzenia). Zwraca true przy powodzeniu.
    pub fn export(
        &mut self,
        patch_id: &str,
        expected_revision: i64,
        destination_path: &str,
    ) -> bool {
        self.exec(&Command::ExportPatch {
            target: Self::library_target(patch_id, expected_revision),
            destination_path: destination_path.to_owned(),
        })
        .is_some()
    }

    /// Importuje plik `.mg101patch` (pojedynczy lub zestaw 36) do biblioteki.
    /// Zwraca liczbę zaimportowanych patchy (0 przy błędzie).
    pub fn import(&mut self, path: &str) -> usize {
        match self.exec(&Command::ImportPatch {
            path: path.to_owned(),
        }) {
            Some(v) => v
                .get("importedPatchIDs")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0),
            None => 0,
        }
    }

    fn localize_error(&self, e: &ExecError) -> String {
        // Prefiks zlokalizowany + treść techniczna (spójne z alertem v1).
        format!("{}: {e}", tr(self.lang, "error.title"))
    }
}

/// Domyślna konfiguracja agenta — Anthropic, endpoint publiczny, model bieżący;
/// klucz pusty (użytkownik uzupełnia w ustawieniach).
fn default_agent_config() -> AgentConfig {
    AgentConfig {
        provider: Provider::Anthropic,
        endpoint: "https://api.anthropic.com".into(),
        model: "claude-sonnet-5".into(),
        api_key: String::new(),
    }
    .normalized()
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
        assert!(vm.set_bpm("p1", before, 123));
        let d = vm.detail("p1").unwrap();
        assert_eq!(d.bpm, 123);
        assert_eq!(
            d.revision,
            before + 1,
            "rewizja podbita (optimistic concurrency)"
        );
    }

    #[test]
    fn stale_revision_is_rejected_as_conflict() {
        // K1: edycja z rewizją, której użytkownik już nie widzi (bo ktoś zmienił
        // patch w międzyczasie), MUSI się nie powieść — nie po cichu nadpisać.
        let mut vm = vm();
        let rev = vm.detail("p1").unwrap().revision;
        assert!(vm.set_bpm("p1", rev, 100)); // pierwszy zapis podbija rewizję
        vm.clear_error();
        // Drugi zapis ze STARĄ rewizją (rev, nie rev+1) → konflikt.
        assert!(
            !vm.set_bpm("p1", rev, 200),
            "stara rewizja nie powinna przejść"
        );
        assert!(vm.last_error().is_some(), "konflikt zgłoszony");
        assert_eq!(vm.detail("p1").unwrap().bpm, 100, "wartość nietknięta");
    }

    #[test]
    fn edit_bypass_persists() {
        let mut vm = vm();
        let d = vm.detail("p1").unwrap();
        let block = d.blocks[0].block.clone();
        assert!(vm.set_bypass("p1", d.revision, &block, true));
        assert!(vm.detail("p1").unwrap().blocks[0].bypassed);
    }

    #[test]
    fn edit_parameter_persists_after_selecting_model_with_params() {
        // Zerowy blob → model 0 bywa bez parametrów; najpierw ustaw model, który
        // parametry ma, potem edytuj jego parametr (realny test set_parameter).
        let (profile, catalog) = mg101_pack_nux_mg101::load().unwrap();
        let (block_id, model) = profile
            .blocks
            .iter()
            .find_map(|b| {
                catalog
                    .models(&b.id)
                    .into_iter()
                    .find(|m| !m.parameters.is_empty())
                    .map(|m| (b.id.clone(), m.clone()))
            })
            .expect("katalog MG-101 ma model z parametrami");

        let mut vm = vm();
        let rev = vm.detail("p1").unwrap().revision;
        let values: Vec<i64> = model.parameters.iter().map(|p| p.minimum()).collect();
        assert!(vm.set_model("p1", rev, &block_id, model.model_id, values, false));

        let d = vm.detail("p1").unwrap();
        let blk = d.blocks.iter().find(|b| b.block == block_id).unwrap();
        let p = &blk.parameters[0];
        let target = (p.minimum + p.maximum) / 2;
        assert!(vm.set_parameter("p1", d.revision, &block_id, &p.name, target));
        let after = vm.detail("p1").unwrap();
        let blk2 = after.blocks.iter().find(|b| b.block == block_id).unwrap();
        assert_eq!(blk2.parameters[0].value, target, "parametr zapisany");
    }

    #[test]
    fn duplicate_creates_new_patch_and_delete_removes() {
        let mut vm = vm();
        let rev = vm.detail("p1").unwrap().revision;
        let new_id = vm.duplicate("p1", rev).unwrap();
        assert_ne!(new_id, "p1");
        assert_eq!(vm.library_rows().len(), 3);
        let new_rev = vm.detail(&new_id).unwrap().revision;
        assert!(vm.delete(&new_id, new_rev));
        assert_eq!(vm.library_rows().len(), 2);
    }

    #[test]
    fn changes_reports_byte_diffs_after_edit() {
        let mut vm = vm();
        let rev = vm.detail("p1").unwrap().revision;
        assert!(vm.set_bpm("p1", rev, 140));
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
    fn models_lists_block_models_and_switch_changes_active() {
        let mut vm = vm();
        // Znajdź blok, który ma >1 model i model z parametrami.
        let (profile, catalog) = mg101_pack_nux_mg101::load().unwrap();
        let block = profile
            .blocks
            .iter()
            .find(|b| catalog.models(&b.id).len() > 1)
            .map(|b| b.id.clone())
            .expect("blok z >1 modelem");
        let opts = vm.models(&block);
        assert!(opts.len() > 1, "picker ma listę modeli");

        let rev = vm.detail("p1").unwrap().revision;
        let cur = vm
            .detail("p1")
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.block == block)
            .unwrap()
            .model_id;
        let target = opts.iter().find(|o| o.id != cur).unwrap().id;
        assert!(vm.switch_model("p1", rev, &block, target));
        let after = vm
            .detail("p1")
            .unwrap()
            .blocks
            .iter()
            .find(|b| b.block == block)
            .unwrap()
            .model_id;
        assert_eq!(after, target, "model bloku przełączony");
    }

    #[test]
    fn export_writes_patch_file() {
        let mut vm = vm();
        let (profile, _) = mg101_pack_nux_mg101::load().unwrap();
        let dir = std::env::temp_dir().join(format!("mg101_vm_export_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let dest = dir.join("out.mg101patch");
        let _ = std::fs::remove_file(&dest);
        let rev = vm.detail("p1").unwrap().revision;
        assert!(vm.export("p1", rev, dest.to_str().unwrap()));
        let written = std::fs::read(&dest).unwrap();
        assert_eq!(
            written.len(),
            profile.record_size,
            "eksport = rozmiar rekordu"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_adds_patches_from_file() {
        let mut vm = vm();
        let (profile, _) = mg101_pack_nux_mg101::load().unwrap();
        let dir = std::env::temp_dir().join(format!("mg101_vm_import_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("two.mg101patch");
        // Dwa rekordy (wielokrotność record_size) → dwa patche.
        std::fs::write(&path, vec![0u8; profile.record_size * 2]).unwrap();
        let n = vm.import(path.to_str().unwrap());
        assert_eq!(n, 2);
        assert_eq!(vm.library_rows().len(), 4); // 2 startowe + 2 zaimportowane
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_config_defaults_and_updates() {
        let mut vm = vm();
        assert_eq!(vm.agent_config().provider, Provider::Anthropic);
        assert!(!vm.is_agent_configured(), "brak klucza → nieskonfigurowany");
        let mut cfg = vm.agent_config().clone();
        cfg.api_key = "sk-test".into();
        vm.set_agent_config(cfg);
        assert!(vm.is_agent_configured(), "z kluczem → skonfigurowany");
    }

    #[test]
    fn chat_rows_and_usage_accounting() {
        use mg101_agent_core::{ChatMessage, Usage};
        let mut vm = vm();
        vm.push_user_message("ustaw bpm na 120");
        // Symuluj wynik przebiegu: user + asystent, zużycie tokenów.
        let history = vec![
            ChatMessage::user("ustaw bpm na 120"),
            ChatMessage::assistant("Zrobione."),
        ];
        vm.apply_run_outcome(
            history,
            Usage {
                input_tokens: 1000,
                output_tokens: 500,
            },
        );
        let rows = vm.chat_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].role, "user");
        assert_eq!(rows[1].role, "assistant");
        assert_eq!(vm.session_usage().input_tokens, 1000);
        // Koszt: model domyślny (sonnet) ma cennik → koszt > 0.
        assert!(vm.session_cost_usd() > 0.0, "koszt sesji z cennika");
    }

    #[test]
    fn execute_tool_routes_to_studio() {
        let mut vm = vm();
        // Narzędzie odczytu przez ścieżkę agenta (execute_tool) zwraca JSON.
        let v = vm.execute_tool(&Command::ListPatches).unwrap();
        assert!(v.as_array().is_some());
        // Błąd narzędzia → String (kształt dla modelu).
        let err = vm.execute_tool(&Command::GetPatch {
            patch_id: "nope".into(),
        });
        assert!(err.is_err());
    }

    #[test]
    fn revert_last_undoes_edit() {
        let mut vm = vm();
        let rev = vm.detail("p1").unwrap().revision;
        assert!(vm.set_bpm("p1", rev, 200));
        assert_eq!(vm.detail("p1").unwrap().bpm, 200);
        assert!(vm.revert_last());
        assert_ne!(
            vm.detail("p1").unwrap().bpm,
            200,
            "revert cofnął zmianę BPM"
        );
    }
}

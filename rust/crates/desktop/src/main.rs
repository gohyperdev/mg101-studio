//! Binarka desktop MG101 Studio — okno Slint spięte z [`ViewModel`].
//!
//! UI (Slint) jest cienką powłoką: renderuje struktury z VM i woła jego metody.
//! Cała logika i stan są w [`mg101_desktop::vm`] (te same komendy co agent/MCP).
//! Agent biegnie na osobnym wątku ([`mg101_desktop::agent`]); jego narzędzia
//! wracają na wątek UI przez Timer, więc `Studio` pozostaje jednowątkowe.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use mg101_agent_core::{AgentConfig, Provider};
use mg101_core::wal::{JournalStore, TransactionEntry, WalError};
use mg101_desktop::agent::{AgentEvent, ChatRunner, SYSTEM_PROMPT};
use mg101_desktop::vm::LibraryTab;
use mg101_desktop::{Lang, ViewModel};
use mg101_library::SqliteStore;
use mg101_studio::Studio;
use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

type Vm = ViewModel<SqliteStore>;

/// Zdekodowane rekordy slotów urządzenia: (bank, index) → (nazwa, bajty plikowe).
type SlotRecords = Rc<RefCell<std::collections::HashMap<(String, u16), (String, Vec<u8>)>>>;

/// Katalog danych aplikacji (trwała Library) — parytet v1.
/// macOS: `~/Library/Application Support/dev.mos.mg101studio`;
/// Windows: `%APPDATA%\dev.mos.mg101studio`; inne: `$HOME/.mg101studio`.
fn app_data_dir() -> std::path::PathBuf {
    const APP_DIR: &str = "dev.mos.mg101studio";
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home)
                .join("Library/Application Support")
                .join(APP_DIR);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return std::path::Path::new(&appdata).join(APP_DIR);
        }
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    std::path::Path::new(&home).join(".mg101studio")
}

/// Dziennik WAL sesji w pamięci — daje działający revert (review E7/K2).
#[derive(Default)]
struct SessionJournal {
    entries: RefCell<Vec<TransactionEntry>>,
}

impl JournalStore for SessionJournal {
    fn append(&self, entry: &TransactionEntry) -> Result<(), WalError> {
        self.entries.borrow_mut().push(entry.clone());
        Ok(())
    }
    fn load(&self) -> Result<Vec<TransactionEntry>, WalError> {
        Ok(self.entries.borrow().clone())
    }
    fn rewrite(&self, entries: &[TransactionEntry]) -> Result<(), WalError> {
        *self.entries.borrow_mut() = entries.to_vec();
        Ok(())
    }
}

fn tab_index(tab: LibraryTab) -> i32 {
    match tab {
        LibraryTab::User => 0,
        LibraryTab::Factory => 1,
        LibraryTab::Library => 2,
    }
}

fn tab_from_index(i: i32) -> LibraryTab {
    match i {
        0 => LibraryTab::User,
        1 => LibraryTab::Factory,
        _ => LibraryTab::Library,
    }
}

/// Etykieta slotu w konwencji footswitcha MG-101/QuickTone: 9 banków × 4 litery
/// (A–D), więc index 0→"1A", 1→"1B", 4→"2A" … 35→"9D". Slot spoza 0..35 zwraca
/// numer surowy (bezpieczne dla nietypowych profili).
fn slot_label(index: u16) -> String {
    const LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];
    if index >= 36 {
        return index.to_string();
    }
    format!("{}{}", index / 4 + 1, LETTERS[(index % 4) as usize])
}

/// Index slotu User dla patcha otwartego z urządzenia (`device-user-<n>`), jeśli
/// to taki patch. Służy do bramkowania edycji na żywo: CC wysyłamy tylko, gdy
/// edytowany patch jest AKTYWNYM presetem User na urządzeniu.
fn device_user_index(id: &str) -> Option<i32> {
    id.strip_prefix("device-user-")?.parse().ok()
}

/// Czytelny opis komunikatu MIDI do monitora (CC/PC/SysEx + hex).
fn describe_midi(msg: &[u8]) -> String {
    let hex: Vec<String> = msg.iter().map(|b| format!("{b:02X}")).collect();
    let hex = hex.join(" ");
    let tag = match msg.first() {
        Some(&s) if (0xB0..=0xBF).contains(&s) && msg.len() >= 3 => {
            format!("CC{} = {} (kan {})", msg[1], msg[2], s & 0x0F)
        }
        Some(&s) if (0xC0..=0xCF).contains(&s) && msg.len() >= 2 => {
            format!("PC {} (kan {})", msg[1], s & 0x0F)
        }
        Some(0xF0) => format!("SysEx {} B", msg.len()),
        _ => "inny".to_string(),
    };
    format!("{tag:20}  [{hex}]")
}

fn empty_detail() -> DetailUi {
    DetailUi {
        id: SharedString::new(),
        name: SharedString::new(),
        bpm: 0,
        revision: 0,
        ir: false,
        blocks: ModelRc::new(VecModel::from(Vec::<BlockRowUi>::new())),
    }
}

/// Generyczna ikona typu bloku (nie modelu) — symbolizuje rodzaj efektu w łańcuchu.
/// Świadomie NIE odwzorowujemy skeuomorficznej grafiki per model z QuickTone
/// (kolory/kształty/napisy konkretnego pedału) — to bajer bez wartości edycyjnej;
/// nasze podejście listy parametrów (jak ToneBridge) wystarcza. Zero zasobów, IP-safe.
fn block_icon(block: &str) -> &'static str {
    match block {
        "wah" => "👄",  // wah/filtr
        "cmp" => "🗜️",  // kompresor
        "efx" => "⚡",  // boost/drive
        "amp" => "🔊",  // wzmacniacz
        "eq" => "📊",   // korektor (pasma)
        "gate" => "🚪", // bramka szumów
        "mod" => "🌀",  // modulacja
        "dly" => "🔁",  // delay/echo
        "rvb" => "🌊",  // pogłos
        "cab" => "📦",  // kolumna/IR
        "sr" => "🔈",   // wyjście/poziom
        _ => "🎛️",     // nieznany typ
    }
}

/// Buduje widok szczegółów patcha, dociągając modele bloków (picker) z VM.
fn detail_to_ui(vm: &mut Vm, d: &mg101_desktop::PatchDetail) -> DetailUi {
    let blocks: Vec<BlockRowUi> = d
        .blocks
        .iter()
        .map(|b| {
            let params: Vec<ParamRowUi> = b
                .parameters
                .iter()
                .map(|p| ParamRowUi {
                    name: p.name.clone().into(),
                    label: p.label.clone().into(),
                    value: p.value as i32,
                    minimum: p.minimum as i32,
                    maximum: p.maximum as i32,
                    control: p.control.clone().into(),
                    unit: p.unit.clone().into(),
                    midi_cc: p.midi_cc as i32,
                    confirmed: p.confirmed,
                    display: p.display.clone().into(),
                    enum_label: p.enum_label.clone().into(),
                    enum_next: p.enum_next as i32,
                    enum_next_label: p.enum_next_label.clone().into(),
                    enum_labels: ModelRc::new(VecModel::from(
                        p.enum_labels
                            .iter()
                            .map(|s| SharedString::from(s.as_str()))
                            .collect::<Vec<_>>(),
                    )),
                    enum_values: ModelRc::new(VecModel::from(
                        p.enum_values.iter().map(|&v| v as i32).collect::<Vec<_>>(),
                    )),
                    enum_index: p.enum_index as i32,
                    addr: if p.offset >= 0 {
                        format!("0x{:04x}", p.offset).into()
                    } else {
                        SharedString::new()
                    },
                })
                .collect();
            let opts = vm.models(&b.block);
            let names: Vec<SharedString> = opts.iter().map(|o| o.name.clone().into()).collect();
            let idx = opts.iter().position(|o| o.id == b.model_id).unwrap_or(0) as i32;
            BlockRowUi {
                block: b.block.clone().into(),
                icon: block_icon(&b.block).into(),
                model_name: b.model_name.clone().into(),
                bypassed: b.bypassed,
                params: ModelRc::new(VecModel::from(params)),
                models: ModelRc::new(VecModel::from(names)),
                model_index: idx,
            }
        })
        .collect();
    DetailUi {
        id: d.patch_id.clone().into(),
        name: d.name.clone().into(),
        bpm: d.bpm as i32,
        revision: d.revision as i32,
        ir: d.ir_present,
        blocks: ModelRc::new(VecModel::from(blocks)),
    }
}

/// Ustawia etykiety i18n na oknie (po starcie i po zmianie języka).
fn apply_labels(ui: &AppWindow, vm: &Vm) {
    let l = |k: &str| SharedString::from(vm.label(k));
    ui.set_t_app_title(l("app.title"));
    ui.set_t_tab_user(l("tab.user"));
    ui.set_t_tab_factory(l("tab.factory"));
    ui.set_t_tab_library(l("tab.library"));
    ui.set_t_no_selection(l("editor.no_selection"));
    ui.set_t_chain(l("editor.chain"));
    ui.set_t_bypass(l("editor.bypass"));
    ui.set_t_name(l("editor.name"));
    ui.set_t_bpm(l("editor.bpm"));
    ui.set_t_changes(l("inspector.changes"));
    ui.set_t_changes_header(l("inspector.changes_header"));
    ui.set_t_binary(l("inspector.binary"));
    ui.set_t_agent(l("inspector.agent"));
    ui.set_t_mcp(l("inspector.mcp"));
    ui.set_t_no_changes(l("inspector.no_changes"));
    ui.set_t_duplicate(l("action.duplicate"));
    ui.set_t_delete(l("action.delete"));
    ui.set_t_revert(l("action.revert"));
    ui.set_t_import(l("action.import"));
    ui.set_t_export(l("action.export"));
    ui.set_t_language(l("settings.language"));
    ui.set_t_fetch(l("device.fetch"));
    ui.set_t_import_dump(l("device.import_dump"));
    ui.set_t_copy_library(l("device.copy_to_library"));
    ui.set_t_tip_value(l("tip.value"));
    ui.set_t_tip_inc(l("tip.inc"));
    ui.set_t_tip_dec(l("tip.dec"));
    ui.set_t_tip_import(l("tip.import"));
    ui.set_t_tip_export(l("tip.export"));
    ui.set_t_tip_duplicate(l("tip.duplicate"));
    ui.set_t_tip_delete(l("tip.delete"));
    ui.set_t_tip_revert(l("tip.revert"));
    ui.set_t_tip_copy_library(l("tip.copy_library"));
    ui.set_t_tip_toggle(l("tip.toggle"));
    ui.set_t_empty_slot(l("slot.empty"));
    ui.set_t_rev(l("editor.rev"));
    ui.set_t_settings(l("inspector.settings"));
    ui.set_t_provider(l("settings.provider"));
    ui.set_t_endpoint(l("settings.endpoint"));
    ui.set_t_model(l("settings.model"));
    ui.set_t_key(l("settings.key"));
    ui.set_t_save(l("action.save"));
    ui.set_t_send(l("agent.send"));
    // Sekcje urządzenia / MIDI / DRUM / Meta / Źródła.
    ui.set_t_qt_warning(l("device.qt_warning"));
    ui.set_t_connect_hint(l("device.connect_hint"));
    ui.set_t_connect_control(l("device.connect_to_control"));
    ui.set_t_slot_hint(l("slot.hint"));
    ui.set_t_fields(l("inspector.fields"));
    ui.set_t_role_tool(l("chat.role_tool"));
    ui.set_t_role_assistant(l("chat.role_assistant"));
    // agent.working / agent.tool składane w agent_status_text (nie jako gotowa etykieta).
    ui.set_t_clear(l("action.clear"));
    ui.set_t_save_file(l("action.save_file"));
    ui.set_t_open(l("action.open"));
    ui.set_t_add(l("action.add"));
    ui.set_t_create(l("action.create"));
    ui.set_t_set(l("action.set"));
    ui.set_t_midi_listen(l("midi.listen"));
    ui.set_t_midi_active(l("midi.active"));
    ui.set_t_midi_idle(l("midi.idle"));
    ui.set_t_midi_no_device(l("midi.no_device"));
    ui.set_t_midi_empty(l("midi.empty"));
    ui.set_t_drum_hint(l("drum.hint"));
    ui.set_t_drum_tempo(l("drum.tempo"));
    ui.set_t_drum_volume(l("drum.volume"));
    ui.set_t_drum_group(l("drum.group"));
    ui.set_t_drum_pattern(l("drum.pattern"));
    ui.set_t_meta(l("inspector.meta"));
    ui.set_t_meta_no_selection(l("meta.no_selection"));
    ui.set_t_meta_author(l("meta.author"));
    ui.set_t_meta_source(l("meta.source"));
    ui.set_t_meta_source_url(l("meta.source_url"));
    ui.set_t_meta_license(l("meta.license"));
    ui.set_t_meta_notes(l("meta.notes"));
    ui.set_t_meta_rating(l("meta.rating"));
    ui.set_t_meta_no_rating(l("meta.no_rating"));
    ui.set_t_meta_favorite(l("meta.favorite"));
    ui.set_t_meta_save(l("meta.save"));
    ui.set_t_meta_tags(l("meta.tags"));
    ui.set_t_meta_collections(l("meta.collections"));
    ui.set_t_ph_author(l("meta.ph_author"));
    ui.set_t_ph_source(l("meta.ph_source"));
    ui.set_t_ph_license(l("meta.ph_license"));
    ui.set_t_ph_tag(l("meta.ph_tag"));
    ui.set_t_ph_collection(l("meta.ph_collection"));
    ui.set_t_sources(l("inspector.sources"));
    ui.set_t_sources_intro(l("sources.intro"));
    ui.set_t_sources_paid(l("sources.paid"));
    ui.set_t_sources_author(l("sources.author"));
    ui.set_t_sources_license(l("sources.license"));
    ui.set_t_sources_open(l("sources.open"));
    // Katalog źródeł to DANE, ale widoczne w UI — opis i licencja też są tłumaczone,
    // więc lista musi być przebudowana przy każdej zmianie języka.
    apply_sources(ui, vm.lang());
    // Etykieta urządzenia zawiera tłumaczone „brak urządzenia" — odśwież przy zmianie języka.
    if !ui.get_device_connected() {
        ui.set_device_label(l("device.none"));
    }
    ui.set_lang_index(if vm.lang() == Lang::Pl { 1 } else { 0 });
}

/// Wypełnia zakładkę Źródła katalogiem w bieżącym języku.
fn apply_sources(ui: &AppWindow, lang: Lang) {
    let items: Vec<SourceUi> = mg101_desktop::sources::all(lang)
        .into_iter()
        .map(|s| SourceUi {
            name: s.name.into(),
            author: s.author.into(),
            license: s.license.into(),
            url: s.url.into(),
            paid: s.paid,
            description: s.description.into(),
        })
        .collect();
    ui.set_patch_sources(ModelRc::new(VecModel::from(items)));
}

/// Odświeża dane na oknie z bieżącego stanu VM (nie dotyka `agent-busy`).
/// Wypełnia zakładkę Meta danymi otwartego patcha (autorstwo, tagi, kolekcje).
/// Kolekcje pobieramy tu, bo `member` zależy od BIEŻĄCEGO patcha.
fn apply_meta(ui: &AppWindow, vm: &mut Vm, d: &mg101_desktop::PatchDetail) {
    let m = &d.meta;
    ui.set_meta_author(m.author.clone().into());
    ui.set_meta_source(m.source.clone().into());
    ui.set_meta_source_url(m.source_url.clone().into());
    ui.set_meta_license(m.license.clone().into());
    ui.set_meta_notes(m.notes.clone().into());
    ui.set_meta_rating(m.rating as i32);
    ui.set_meta_favorite(m.favorite);
    let tags: Vec<SharedString> = m.tags.iter().map(|t| t.clone().into()).collect();
    ui.set_meta_tags(ModelRc::new(VecModel::from(tags)));

    let cols: Vec<CollectionUi> = vm
        .collections()
        .into_iter()
        .map(|(id, name, count)| CollectionUi {
            member: m.collections.contains(&id),
            id: id.into(),
            name: name.into(),
            count: count as i32,
        })
        .collect();
    ui.set_collections(ModelRc::new(VecModel::from(cols)));
}

fn refresh(ui: &AppWindow, vm: &mut Vm) {
    ui.set_active_tab(tab_index(vm.tab()));

    let rows: Vec<PatchRowUi> = vm
        .library_rows()
        .iter()
        .map(|r| PatchRowUi {
            id: r.patch_id.clone().into(),
            slot: r.slot as i32,
            name: r.name.clone().into(),
            origin: r.origin.clone().into(),
            ir: r.ir_present,
        })
        .collect();
    ui.set_patches(ModelRc::new(VecModel::from(rows)));

    let slot_bank = match vm.tab() {
        LibraryTab::Factory => "factory",
        _ => "user",
    };
    let slots: Vec<SlotRowUi> = vm
        .slot_rows()
        .iter()
        .map(|s| SlotRowUi {
            index: s.index as i32,
            label: slot_label(s.index).into(),
            patch_id: format!("device-{slot_bank}-{}", s.index).into(),
            name: s.name.clone().into(),
            occupied: s.occupied,
            writable: s.writable,
        })
        .collect();
    ui.set_slots(ModelRc::new(VecModel::from(slots)));

    match vm.selected_id() {
        Some(id) => {
            ui.set_has_selection(true);
            ui.set_current_id(id.clone().into());
            match vm.detail(&id) {
                Some(d) => {
                    apply_meta(ui, vm, &d);
                    let dui = detail_to_ui(vm, &d);
                    ui.set_detail(dui);
                }
                None => ui.set_detail(empty_detail()),
            }
            let changes = vm.changes(&id);
            let text = if changes.is_empty() {
                String::new()
            } else {
                changes
                    .iter()
                    .take(200)
                    .map(|c| format!("@{:#06x}: {} → {}", c.offset, c.before, c.after))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ui.set_changes_text(text.into());

            // Inspektor binarny (W5): grupy logiczne z hex/dec.
            let bin: Vec<BinGroupUi> = vm
                .binary_view(&id)
                .into_iter()
                .map(|g| {
                    let hex = g
                        .bytes
                        .iter()
                        .map(|b| format!("{:02X}", *b as u8))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let dec = g
                        .bytes
                        .iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    BinGroupUi {
                        label: g.label.into(),
                        addr: format!("0x{:04X}", g.offset).into(),
                        kind: g.kind.into(),
                        length: g.length as i32,
                        hex: hex.into(),
                        dec: dec.into(),
                        truncated: g.truncated,
                    }
                })
                .collect();
            ui.set_bin_groups(ModelRc::new(VecModel::from(bin)));
        }
        None => {
            ui.set_has_selection(false);
            ui.set_current_id(SharedString::new());
            ui.set_detail(empty_detail());
            ui.set_changes_text(SharedString::new());
            ui.set_bin_groups(ModelRc::new(VecModel::from(Vec::<BinGroupUi>::new())));
        }
    }

    // Rozmowa agenta + rozliczenie sesji.
    let chat: Vec<ChatRowUi> = vm
        .chat_rows()
        .into_iter()
        .map(|r| ChatRowUi {
            role: r.role.into(),
            text: r.text.into(),
        })
        .collect();
    ui.set_chat_rows(ModelRc::new(VecModel::from(chat)));
    let u = vm.session_usage();
    ui.set_usage_text(
        format!(
            "in {} / out {} tok · ${:.4}",
            u.input_tokens,
            u.output_tokens,
            vm.session_cost_usd()
        )
        .into(),
    );

    // Ustawienia AI.
    let cfg = vm.agent_config();
    ui.set_provider_index(if cfg.provider == Provider::Anthropic {
        0
    } else {
        1
    });
    ui.set_cfg_endpoint(cfg.endpoint.clone().into());
    ui.set_cfg_model(cfg.model.clone().into());
    ui.set_cfg_key(cfg.api_key.clone().into());

    ui.set_error_text(vm.last_error().unwrap_or("").into());
}

/// Wykrywa podłączone urządzenie (po nazwach portów MIDI) i ustawia status w UI.
/// Nie dotyka danych banków — te wypełnia dopiero zrzut.
fn detect_device(ui: &AppWindow, lang: Lang) {
    match mg101_desktop::device::detect() {
        Some(d) => {
            // Producent i model to nazwy własne — nie tłumaczymy.
            ui.set_device_label(format!("{} {}", d.manufacturer, d.model).into());
            ui.set_device_connected(true);
        }
        None => {
            ui.set_device_label(mg101_desktop::i18n::tr(lang, "device.none").into());
            ui.set_device_connected(false);
        }
    }
}

/// Tekst postępu przebiegu agenta: „Agent pracuje… 12 s · narzędzie: list_patches".
/// Przebieg potrafi trwać ~minutę (wiele wywołań narzędzi + duży zestaw definicji),
/// więc bez tego licznika UI wygląda na zawieszony.
fn agent_status_text(lang: Lang, progress: &Option<(std::time::Instant, String)>) -> String {
    let Some((started, tool)) = progress else {
        return String::new();
    };
    let secs = started.elapsed().as_secs();
    let working = mg101_desktop::i18n::tr(lang, "agent.working");
    if tool.is_empty() {
        format!("{working} {secs} s")
    } else {
        let label = mg101_desktop::i18n::tr(lang, "agent.tool");
        format!("{working} {secs} s · {label} {tool}")
    }
}

/// Stan leniwego odczytu klucza API z systemowego magazynu sekretów.
/// Odczyt zdarza się **raz**, przy pierwszym użyciu agenta — nie na starcie aplikacji.
enum KeyLoad {
    /// Jeszcze nie pytaliśmy magazynu.
    Idle,
    /// Odczyt trwa w tle; wynik przyjdzie kanałem.
    Loading(std::sync::mpsc::Receiver<Option<String>>),
    /// Magazyn już odpowiedział (kluczem albo jego brakiem) — nie pytamy ponownie.
    Done,
}

/// Startuje odczyt klucza z magazynu, jeśli jeszcze go nie mamy i nie trwa.
/// Idempotentne: kolejne wywołania nic nie robią (żadnego drugiego okna Keychain).
fn ensure_agent_key(vm: &Vm, state: &Rc<RefCell<KeyLoad>>) {
    let mut s = state.borrow_mut();
    if !matches!(*s, KeyLoad::Idle) {
        return;
    }
    // Klucz wpisany ręcznie w Ustawieniach → magazyn niepotrzebny.
    if !vm.agent_config().api_key.is_empty() {
        *s = KeyLoad::Done;
        return;
    }
    *s = KeyLoad::Loading(mg101_desktop::keychain::load_key_async(
        vm.agent_config().provider,
    ));
}

/// Obsługuje komendy warstwy urządzenia (DRUM na żywo, katalog źródeł patchy),
/// kierując je poza Studio — bo Studio jest device-agnostyczne i nie zna ani mapy CC
/// MG-101, ani listy stron z patchami. Zwraca `None`, gdy komendę ma wykonać Studio.
fn handle_drum_command(
    cmd: &mg101_commands::Command,
    presync: &Rc<RefCell<Option<mg101_desktop::device::PresetSync>>>,
    lang: Lang,
) -> Option<Result<serde_json::Value, String>> {
    use mg101_commands::Command;
    use mg101_desktop::drum;
    use serde_json::json;

    // Katalog publicznych źródeł patchy — same odnośniki (autor/URL/licencja).
    // Aplikacja NIE rozpowszechnia cudzych plików patchy: żadne z tych źródeł nie
    // daje licencji na redystrybucję.
    if let Command::ListPatchSources = cmd {
        let items: Vec<serde_json::Value> = mg101_desktop::sources::all(lang)
            .into_iter()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "author": s.author,
                    "url": s.url,
                    "license": s.license,
                    "paid": s.paid,
                    "description": s.description,
                })
            })
            .collect();
        return Some(Ok(json!({
            "sources": items,
            "note": "Same odnośniki — aplikacja nie zawiera cudzych plików patchy \
                     (brak licencji na redystrybucję). Pobierz od autora i zaimportuj.",
        })));
    }

    // Katalog nie wymaga urządzenia — czysty odczyt danych.
    if let Command::DrumCatalog = cmd {
        let groups: Vec<serde_json::Value> = drum::GROUPS
            .iter()
            .enumerate()
            .map(|(gi, g)| {
                json!({
                    "group": g.name,
                    "patterns": g.patterns.iter().enumerate()
                        .map(|(pi, p)| json!({
                            "number": pi + 1,
                            "name": p,
                            "cc82": drum::group_base(gi) + pi as u8,
                        }))
                        .collect::<Vec<_>>(),
                })
            })
            .collect();
        return Some(Ok(json!({ "groups": groups })));
    }

    // Komendy sterujące wymagają aktywnego połączenia.
    let is_drum = matches!(
        cmd,
        Command::DrumTransport { .. }
            | Command::DrumVolume { .. }
            | Command::DrumPattern { .. }
            | Command::DrumTempo { .. }
    );
    if !is_drum {
        return None;
    }
    let guard = presync.borrow();
    let Some(s) = guard.as_ref() else {
        return Some(Err(
            "urządzenie niepodłączone — najpierw Pobierz/połącz z MG-101".into(),
        ));
    };
    Some(match cmd {
        Command::DrumTransport { playing } => {
            s.send_cc(drum::CC_DRUM_TRANSPORT, if *playing { 1 } else { 0 });
            Ok(json!({ "ok": true, "playing": playing }))
        }
        Command::DrumVolume { value } => {
            let v = (*value).clamp(0, 100) as u8;
            s.send_cc(drum::CC_DRUM_VOLUME, v);
            Ok(json!({ "ok": true, "volume": v }))
        }
        Command::DrumPattern { group, pattern } => match drum::find(group, pattern) {
            Some((gi, pi)) => {
                let cc = drum::group_base(gi) + pi as u8;
                s.send_cc(drum::CC_DRUM_PATTERN, cc);
                Ok(json!({
                    "ok": true,
                    "group": drum::GROUPS[gi].name,
                    "pattern": format!("{:02} {}", pi + 1, drum::GROUPS[gi].patterns[pi]),
                    "cc82": cc,
                }))
            }
            None => Err(format!(
                "nieznany wzorzec: grupa='{group}', wzorzec='{pattern}' (użyj drum_list_patterns)"
            )),
        },
        Command::DrumTempo { bpm } => {
            let clamped = (*bpm).clamp(drum::BPM_MIN as i64, drum::BPM_MAX as i64) as u16;
            s.send_raw(drum::tempo_sysex(clamped));
            Ok(json!({ "ok": true, "bpm": clamped }))
        }
        _ => unreachable!("is_drum przefiltrowane wyżej"),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (profile, catalog) = mg101_pack_nux_mg101::load()?;
    let profile = Box::leak(Box::new(profile));
    let catalog = Box::leak(Box::new(catalog));
    // Trwała Library w katalogu danych aplikacji (parytet v1). Awaria dysku nie
    // może wywrócić startu — spadamy na bazę w pamięci z ostrzeżeniem.
    let data_dir = app_data_dir();
    let store = match std::fs::create_dir_all(&data_dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            SqliteStore::open(data_dir.join("library.sqlite")).map_err(|e| e.to_string())
        }) {
        Ok(store) => {
            eprintln!("Library: {}", data_dir.join("library.sqlite").display());
            store
        }
        Err(e) => {
            eprintln!("Library trwała niedostępna ({e}) — pamięć ulotna na tę sesję.");
            SqliteStore::open_in_memory()?
        }
    };
    let studio =
        Studio::new(store, profile, catalog, 0).with_journal(Box::new(SessionJournal::default()));
    let vm: Rc<RefCell<Vm>> = Rc::new(RefCell::new(ViewModel::new(studio, Lang::En)));
    let runner: Rc<RefCell<Option<ChatRunner>>> = Rc::new(RefCell::new(None));
    // Postęp przebiegu agenta: kiedy ruszył i jakie narzędzie właśnie woła.
    // Bez tego UI milczał kilkadziesiąt sekund i wyglądał na zawieszony.
    let agent_progress: Rc<RefCell<Option<(std::time::Instant, String)>>> =
        Rc::new(RefCell::new(None));

    // Klucz API z systemowego magazynu (parytet v1 KeychainStore) — ODCZYT LENIWY.
    // NIE czytamy go na starcie: na macOS pierwszy dostęp do Keychain potrafi
    // wyświetlić okno zgody, a użytkownik, który nie korzysta z agenta, nie powinien
    // go w ogóle oglądać. Odczyt startuje przy pierwszym użyciu agenta (wysłanie
    // wiadomości) albo wejściu w zakładkę Agent/Ustawienia — patrz `ensure_agent_key`.
    // Odczyt biegnie w tle; wynik odbieramy w pompie (nie blokujemy wątku UI).
    let key_load: Rc<RefCell<KeyLoad>> = Rc::new(RefCell::new(KeyLoad::Idle));
    // Wiadomość czekająca na klucz: agent startuje dopiero, gdy magazyn odpowie.
    let pending_run: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

    // Zasianie Biblioteki przy pierwszym starcie (pusta) — 36 patchy fabrycznych,
    // parytet v1. Trwałe (SqliteStore), więc dzieje się raz. Bez sprzętu.
    {
        let mut vmb = vm.borrow_mut();
        if vmb.library_rows().is_empty() {
            let seed_path = data_dir.join("factory-seed.mg101patch");
            match std::fs::write(&seed_path, mg101_pack_nux_mg101::FACTORY_PATCHES) {
                Ok(()) => {
                    let n = vmb.import(&seed_path.to_string_lossy());
                    eprintln!("Biblioteka zasiana: {n} patchy fabrycznych.");
                }
                Err(e) => eprintln!("Nie zasiano Biblioteki ({e}) — użyj Import."),
            }
        }
    }

    let ui = AppWindow::new()?;
    apply_labels(&ui, &vm.borrow());
    refresh(&ui, &mut vm.borrow_mut());
    // Dropdown urządzeń w nagłówku (W6) — modele z rejestru (dziś MG-101; pod kolejne).
    let device_names: Vec<SharedString> = mg101_desktop::device::registry()
        .iter()
        .map(|d| format!("{} {}", d.manufacturer, d.model).into())
        .collect();
    ui.set_device_models(ModelRc::new(VecModel::from(device_names)));
    ui.set_device_index(0);
    // Katalog wzorców DRUM (dwupoziomowy wybór grupa → wzorzec).
    {
        use mg101_desktop::drum;
        let names: Vec<SharedString> =
            drum::GROUPS.iter().map(|g| g.name.into()).collect();
        let patterns: Vec<ModelRc<SharedString>> = drum::GROUPS
            .iter()
            .map(|g| {
                let items: Vec<SharedString> = g
                    .patterns
                    .iter()
                    .enumerate()
                    .map(|(i, p)| format!("{:02} {}", i + 1, p).into())
                    .collect();
                ModelRc::new(VecModel::from(items))
            })
            .collect();
        let bases: Vec<i32> = (0..drum::GROUPS.len())
            .map(|gi| drum::group_base(gi) as i32)
            .collect();
        ui.set_drum_group_names(ModelRc::new(VecModel::from(names)));
        ui.set_drum_group_patterns(ModelRc::new(VecModel::from(patterns)));
        ui.set_drum_group_base(ModelRc::new(VecModel::from(bases)));
    }
    detect_device(&ui, vm.borrow().lang());
    // Uchwyt zrzutu w tle (W2) — Some tylko podczas trwającego zrzutu.
    let dumper: Rc<RefCell<Option<mg101_desktop::device::Dumper>>> = Rc::new(RefCell::new(None));
    // Ostatni zrzut zdekodowany do rekordów plikowych (8402 B × zajęte sloty),
    // gotowy do importu do Biblioteki. Pusty, dopóki nie ma zrzutu.
    let last_dump: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    // Zdekodowane rekordy per slot (bank, index) → (nazwa, bajty plikowe) — do
    // otwierania pojedynczego slotu w edytorze po kliknięciu.
    let slot_records: SlotRecords = Rc::new(RefCell::new(std::collections::HashMap::new()));
    // Trwała sesja synchronizacji presetu (Program Change w obie strony). Some,
    // gdy urządzenie jest podłączone i łącze otwarte. Nasłuch footswitcha + wysyłka
    // wyboru z aplikacji.
    let presync: Rc<RefCell<Option<mg101_desktop::device::PresetSync>>> =
        Rc::new(RefCell::new(None));
    // Aktywny preset na urządzeniu (index User 0..35, -1 = nieznany) — mirror
    // `ui.active-slot`, aktualizowany z footswitcha i wyboru w aplikacji.
    let active_slot: Rc<RefCell<i32>> = Rc::new(RefCell::new(-1));
    // Czy pierwszy automatyczny Fetch wykonano dla bieżącego połączenia (reset przy
    // odłączeniu). Po wykryciu urządzenia pobieramy banki raz, bez klikania.
    let auto_fetched: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    // Monitor MIDI (zakładka MIDI): włącznik + bufor linii (do wyświetlenia/zapisu).
    let midi_mon_on: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let midi_log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    // Makro spinające callback z VM: pożycza VM, wykonuje, odświeża okno.
    macro_rules! wire {
        ($setter:ident, |$vm:ident $(, $arg:ident)*| $body:block) => {{
            let uw = ui.as_weak();
            let vmc = vm.clone();
            ui.$setter(move |$($arg),*| {
                let Some(ui) = uw.upgrade() else { return };
                let mut $vm = vmc.borrow_mut();
                $body
                refresh(&ui, &mut $vm);
            });
        }};
    }

    wire!(on_switch_tab, |vm, i| {
        vm.set_tab(tab_from_index(i));
    });
    wire!(on_select, |vm, id| {
        vm.select(&id);
    });
    wire!(on_duplicate, |vm, id, rev| {
        vm.duplicate(&id, rev as i64);
    });
    wire!(on_delete_patch, |vm, id, rev| {
        vm.delete(&id, rev as i64);
    });
    wire!(on_revert, |vm| {
        vm.revert_last();
    });
    wire!(on_dismiss_error, |vm| {
        vm.clear_error();
    });
    // Import/eksport: modalny dialog `rfd` kręci ZAGNIEŻDŻONĄ pętlę zdarzeń UI,
    // w której może odpalić tick pompy agenta. Dlatego ścieżkę pobieramy PRZED
    // pożyczeniem VM — inaczej pompa zrobiłaby drugie `borrow_mut` i panika
    // (review E7-parytet/K1). `wire!` tu nieużywane (pożycza VM zbyt wcześnie).
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        ui.on_import(move || {
            let Some(ui) = uw.upgrade() else { return };
            let picked = rfd::FileDialog::new()
                .add_filter("MG-101 patch", &["mg101patch"])
                .pick_file();
            if let Some(path) = picked {
                vmc.borrow_mut().import(&path.to_string_lossy());
            }
            refresh(&ui, &mut vmc.borrow_mut());
        });
    }
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        ui.on_export_patch(move |id, rev| {
            let Some(ui) = uw.upgrade() else { return };
            let picked = rfd::FileDialog::new()
                .add_filter("MG-101 patch", &["mg101patch"])
                .set_file_name("patch.mg101patch")
                .save_file();
            if let Some(path) = picked {
                vmc.borrow_mut()
                    .export(&id, rev as i64, &path.to_string_lossy());
            }
            refresh(&ui, &mut vmc.borrow_mut());
        });
    }
    wire!(on_switch_model, |vm, id, rev, block, idx| {
        let opts = vm.models(&block);
        if let Some(opt) = opts.get(idx as usize) {
            let model_id = opt.id;
            vm.switch_model(&id, rev as i64, &block, model_id);
        }
    });
    wire!(on_set_name, |vm, id, rev, text| {
        vm.rename(&id, rev as i64, &text);
    });
    wire!(on_set_bpm, |vm, id, rev, text| {
        if let Ok(bpm) = text.trim().parse::<i64>() {
            vm.set_bpm(&id, rev as i64, bpm);
        }
    });
    wire!(on_set_bypass, |vm, id, rev, block, on| {
        vm.set_bypass(&id, rev as i64, &block, on);
    });
    // Edycja parametru — z trybem interaktywnym: po udanej zmianie aktywnego
    // presetu User wysyłamy Control Change na urządzenie (edycja na żywo). Poza
    // `wire!`, bo potrzebujemy uchwytu sesji sync i wartości CC.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let psync = presync.clone();
        let aslot = active_slot.clone();
        ui.on_set_param(move |id, rev, block, param, value, midi_cc| {
            let Some(ui) = uw.upgrade() else { return };
            let raw = value.round() as i64;
            let ok = vmc
                .borrow_mut()
                .set_parameter(&id, rev as i64, &block, &param, raw);
            // Sync na żywo: tylko gdy zapis się powiódł (raw = wartość zapisana,
            // w zakresie), patch to aktywny preset User, i sesja MIDI działa.
            if ok && midi_cc >= 0 && (0..=127).contains(&raw) {
                if let Some(idx) = device_user_index(&id) {
                    if idx == *aslot.borrow() {
                        if let Some(s) = psync.borrow().as_ref() {
                            s.send_cc(midi_cc as u8, raw as u8);
                        }
                    }
                }
            }
            refresh(&ui, &mut vmc.borrow_mut());
        });
    }
    wire!(on_save_settings, |vm, pidx, endpoint, model, key| {
        let provider = if pidx == 1 {
            Provider::OpenAiCompatible
        } else {
            Provider::Anthropic
        };
        vm.set_agent_config(AgentConfig {
            provider,
            endpoint: endpoint.to_string(),
            model: model.to_string(),
            api_key: key.to_string(),
        });
        // Persystencja klucza w magazynie sekretów (parytet v1) — błąd tylko
        // sygnalizujemy, konfiguracja w pamięci i tak działa do końca sesji.
        if let Err(e) = mg101_desktop::keychain::save_key(provider, &key) {
            vm.report_error(format!("zapis klucza do magazynu: {e}"));
        }
    });

    // Połącz i zrzuć banki (W2): startuje wątek zrzutu dla wykrytego urządzenia.
    {
        let uw = ui.as_weak();
        let dslot = dumper.clone();
        ui.on_connect_dump(move || {
            let Some(ui) = uw.upgrade() else { return };
            if dslot.borrow().is_some() {
                return; // zrzut już trwa
            }
            let Some(dev) = mg101_desktop::device::detect() else {
                ui.set_dump_status("urządzenie odłączone".into());
                ui.set_device_connected(false);
                return;
            };
            ui.set_dump_busy(true);
            ui.set_dump_status("łączenie…".into());
            *dslot.borrow_mut() = Some(mg101_desktop::device::Dumper::start(
                dev.port_needle,
                dev.slots_per_bank,
            ));
        });
    }

    // Import zrzutu do Biblioteki (W2): zdekodowane rekordy plikowe → plik tymczasowy
    // → istniejąca ścieżka importu (pełne parametry, edytowalne). Poza `wire!` — bajty
    // pobieramy przed pożyczeniem VM, spójnie z importem/eksportem plikowym.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let ldump = last_dump.clone();
        let dir = data_dir.clone();
        ui.on_import_dump(move || {
            let Some(ui) = uw.upgrade() else { return };
            let bytes = ldump.borrow().clone();
            if bytes.is_empty() {
                return;
            }
            let path = dir.join("device-dump.mg101patch");
            let n = match std::fs::write(&path, &bytes) {
                Ok(()) => vmc.borrow_mut().import(&path.to_string_lossy()),
                Err(e) => {
                    vmc.borrow_mut().report_error(format!("zapis zrzutu: {e}"));
                    0
                }
            };
            ui.set_dump_status(format!("zaimportowano {n} patchy do Biblioteki").into());
            refresh(&ui, &mut vmc.borrow_mut());
        });
    }

    // Otwarcie slotu urządzenia w edytorze (W2): dekod już w mapie; wstaw do
    // Biblioteki pod stabilnym id (idempotentnie) i zaznacz → edytor pokazuje
    // pełne szczegóły i pozwala edytować kopię.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let srecs = slot_records.clone();
        let psync = presync.clone();
        let aslot = active_slot.clone();
        ui.on_open_slot(move |bank, index| {
            let Some(ui) = uw.upgrade() else { return };
            let key = (bank.to_string(), index as u16);
            let entry = srecs.borrow().get(&key).cloned();
            if let Some((name, record)) = entry {
                let id = format!("device-{bank}-{index}");
                vmc.borrow_mut().open_device_patch(&id, &name, record);
            } else {
                vmc.borrow_mut()
                    .report_error("brak zdekodowanego slotu — użyj Pobierz w nagłówku".into());
            }
            // Sync app→urządzenie: wybór presetu z banku User wysyła Program Change
            // (footswitch 1A..9D = PC 0..35). Bank Factory nie jest wybierany PC.
            if bank == "user" {
                if let Some(s) = psync.borrow().as_ref() {
                    s.select(index as u16);
                }
                *aslot.borrow_mut() = index;
                ui.set_active_slot(index);
            }
            refresh(&ui, &mut vmc.borrow_mut());
        });
    }

    // Wybór modelu urządzenia z dropdownu (W6). Dziś jeden model + auto-detekcja,
    // więc zapamiętujemy indeks (przygotowanie pod kolejne urządzenia).
    {
        let uw = ui.as_weak();
        ui.on_select_device(move |idx| {
            let Some(ui) = uw.upgrade() else { return };
            ui.set_device_index(idx);
        });
    }

    // Transfer: kopiuje otwarty patch (np. slot urządzenia) do Biblioteki jako
    // trwały wpis i przełącza na zakładkę Biblioteka, żeby efekt był widoczny.
    // Duplikat ma świeże ID, więc nie zostanie nadpisany przy kolejnym zrzucie.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        ui.on_copy_to_library(move || {
            let Some(ui) = uw.upgrade() else { return };
            let (id, rev) = {
                let mut vm = vmc.borrow_mut();
                match vm.selected_id() {
                    Some(id) => {
                        let rev = vm.detail(&id).map(|d| d.revision).unwrap_or(1);
                        (id, rev)
                    }
                    None => return,
                }
            };
            let mut vm = vmc.borrow_mut();
            if let Some(new_id) = vm.duplicate(&id, rev) {
                vm.set_tab(LibraryTab::Library);
                vm.select(&new_id);
            }
            refresh(&ui, &mut vm);
        });
    }

    // Centralne przypisanie EXP: wyślij CC79 = index (0=off..6=RVB) na urządzenie.
    {
        let psync = presync.clone();
        ui.on_set_exp(move |idx| {
            if (0..=6).contains(&idx) {
                if let Some(s) = psync.borrow().as_ref() {
                    s.send_cc(mg101_desktop::device::CC_EXP_TARGET, idx as u8);
                }
            }
        });
    }

    // Generyczne sterowanie CC (DRUM/LOOP): wyślij CC<cc> = <value> na urządzenie.
    {
        let psync = presync.clone();
        ui.on_send_control(move |cc, value| {
            if (0..=127).contains(&cc) && (0..=127).contains(&value) {
                if let Some(s) = psync.borrow().as_ref() {
                    s.send_cc(cc as u8, value as u8);
                }
            }
        });
    }

    // DRUM tempo: wyślij ramkę SysEx BPM na urządzenie.
    {
        let psync = presync.clone();
        ui.on_send_drum_tempo(move |bpm| {
            if bpm > 0 {
                if let Some(s) = psync.borrow().as_ref() {
                    s.send_raw(mg101_desktop::drum::tempo_sysex(bpm as u16));
                }
            }
        });
    }

    // Monitor MIDI: włącz/wyłącz nasłuch wszystkich komunikatów.
    {
        let uw = ui.as_weak();
        let mon = midi_mon_on.clone();
        ui.on_toggle_midi_monitor(move || {
            let Some(ui) = uw.upgrade() else { return };
            let on = !*mon.borrow();
            *mon.borrow_mut() = on;
            ui.set_midi_monitor_on(on);
        });
    }
    // Monitor MIDI: wyczyść bufor.
    {
        let uw = ui.as_weak();
        let mlog = midi_log.clone();
        ui.on_clear_midi_log(move || {
            let Some(ui) = uw.upgrade() else { return };
            mlog.borrow_mut().clear();
            ui.set_midi_log("".into());
        });
    }
    // Monitor MIDI: zapis logu do pliku (dialog rfd — ścieżka poza pożyczeniem VM).
    {
        let uw = ui.as_weak();
        let mlog = midi_log.clone();
        ui.on_save_midi_log(move || {
            let Some(ui) = uw.upgrade() else { return };
            let picked = rfd::FileDialog::new()
                .add_filter("Log tekstowy", &["txt", "log"])
                .set_file_name("midi-monitor.txt")
                .save_file();
            if let Some(path) = picked {
                let body = mlog.borrow().join("\n");
                if let Err(e) = std::fs::write(&path, body) {
                    eprintln!("Zapis logu MIDI nieudany: {e}");
                } else {
                    ui.set_dump_status(format!("zapisano log MIDI: {}", path.display()).into());
                }
            }
        });
    }

    // Zmiana języka: przelicz etykiety + odśwież.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        ui.on_switch_lang(move |idx| {
            let Some(ui) = uw.upgrade() else { return };
            let mut vm = vmc.borrow_mut();
            vm.set_lang(if idx == 1 { Lang::Pl } else { Lang::En });
            apply_labels(&ui, &vm);
            refresh(&ui, &mut vm);
        });
    }

    // --- Metadane / tagi / kolekcje otwartego patcha ---
    wire!(
        on_save_meta,
        |vm, author, source, url, license, notes, rating, favorite| {
            if let Some(id) = vm.selected_id() {
                let rev = vm.detail(&id).map(|d| d.revision).unwrap_or_default();
                vm.set_meta(
                    &id,
                    rev,
                    &author,
                    &source,
                    &url,
                    &license,
                    &notes,
                    rating as i64,
                    favorite,
                );
            }
        }
    );
    wire!(on_add_tag, |vm, tag| {
        if let Some(id) = vm.selected_id() {
            let rev = vm.detail(&id).map(|d| d.revision).unwrap_or_default();
            vm.change_tag(&id, rev, &tag, true);
        }
    });
    wire!(on_remove_tag, |vm, tag| {
        if let Some(id) = vm.selected_id() {
            let rev = vm.detail(&id).map(|d| d.revision).unwrap_or_default();
            vm.change_tag(&id, rev, &tag, false);
        }
    });
    wire!(on_toggle_collection, |vm, collection, add| {
        if let Some(id) = vm.selected_id() {
            let rev = vm.detail(&id).map(|d| d.revision).unwrap_or_default();
            vm.change_collection(&id, rev, &collection, add);
        }
    });
    wire!(on_new_collection, |vm, name| {
        // Nowa kolekcja od razu z otwartym patchem w środku — inaczej użytkownik
        // musiałby ją tworzyć i zaraz zaznaczać, co jest zbędnym krokiem.
        if let Some(cid) = vm.create_collection(&name) {
            if let Some(id) = vm.selected_id() {
                let rev = vm.detail(&id).map(|d| d.revision).unwrap_or_default();
                vm.change_collection(&id, rev, &cid, true);
            }
        }
    });
    // Otwiera link w domyślnej przeglądarce (katalog źródeł / adres autora).
    {
        ui.on_open_url(move |url| {
            let url = url.to_string();
            // Tylko https — nie uruchamiamy dowolnych schematów (file://, itp.).
            if !url.starts_with("https://") {
                eprintln!("Odrzucono link spoza https: {url}");
                return;
            }
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&url).spawn();
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .spawn();
            #[cfg(all(unix, not(target_os = "macos")))]
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        });
    }

    // Leniwy odczyt klucza: wejście w zakładkę agenta/ustawień. Ustawienia MUSZĄ
    // pokazać zapisany klucz — inaczej „Zapisz" z pustym polem skasowałby go z magazynu.
    {
        let vmc = vm.clone();
        let ks = key_load.clone();
        ui.on_ensure_agent_key(move || {
            ensure_agent_key(&vmc.borrow(), &ks);
        });
    }

    // Wysłanie wiadomości do agenta: startuje wątek przebiegu. Gdy klucza jeszcze
    // nie ma, najpierw czytamy go z magazynu, a przebieg rusza w pompie po odczycie.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let slot = runner.clone();
        let ks = key_load.clone();
        let pending = pending_run.clone();
        let progress = agent_progress.clone();
        ui.on_send_message(move |text| {
            let Some(ui) = uw.upgrade() else { return };
            let mut vm = vmc.borrow_mut();
            if slot.borrow().is_some() || *pending.borrow() {
                return; // przebieg już trwa (albo czeka na klucz)
            }
            if vm.is_agent_configured() {
                vm.push_user_message(&text);
                let config = vm.agent_config().clone();
                let history = vm.chat_history();
                *slot.borrow_mut() = Some(ChatRunner::start(
                    config,
                    SYSTEM_PROMPT.to_string(),
                    history,
                ));
                *progress.borrow_mut() = Some((std::time::Instant::now(), String::new()));
                ui.set_agent_busy(true);
                ui.set_agent_status(agent_status_text(vm.lang(), &progress.borrow()).into());
                refresh(&ui, &mut vm);
                return;
            }
            // Brak klucza — spróbuj magazynu (pierwsze użycie agenta).
            ensure_agent_key(&vm, &ks);
            if matches!(*ks.borrow(), KeyLoad::Loading(_)) {
                // Wiadomość ląduje w historii od razu; przebieg wystartuje po odczycie.
                vm.push_user_message(&text);
                *pending.borrow_mut() = true;
                ui.set_agent_busy(true);
            } else {
                vm.report_error("agent nieskonfigurowany (uzupełnij ustawienia AI)".into());
            }
            refresh(&ui, &mut vm);
        });
    }

    // Pompa wątku agenta: obsługuje żądania narzędzi (na wątku UI, na Studio) i
    // zdarzenia końcowe. Timer musi żyć do końca `run()`.
    // Samotest ścieżki agenta w PRAWDZIWEJ aplikacji (bez klikania): wymusza odczyt
    // klucza i wysyła wiadomość, żeby dało się zdiagnozować pompę z terminala.
    let selftest = Timer::default();
    if let Some(msg) = std::env::var_os("MG101_AGENT_SELFTEST") {
        let uw = ui.as_weak();
        selftest.start(TimerMode::SingleShot, Duration::from_millis(600), move || {
            if let Some(ui) = uw.upgrade() {
                eprintln!("[selftest] wymuszam odczyt klucza i wysyłam wiadomość");
                ui.invoke_ensure_agent_key();
                ui.invoke_send_message(msg.to_string_lossy().to_string().into());
            }
        });
    }

    let pump = Timer::default();
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let slot = runner.clone();
        let psync = presync.clone();
        let progress = agent_progress.clone();
        pump.start(TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(ui) = uw.upgrade() else { return };
            if slot.borrow().is_none() {
                return;
            }
            let mut done = false;
            let mut did_work = false;
            {
                let mut vm = vmc.borrow_mut();
                let guard = slot.borrow();
                if let Some(r) = guard.as_ref() {
                    // Wykonaj oczekujące narzędzia na Studio (wątek UI).
                    while let Some(cmd) = r.try_tool_request() {
                        // Komendy DRUM (sterowanie na żywo) idą do urządzenia, nie do Studio.
                        // Pokaż, które narzędzie właśnie leci — inaczej długi przebieg
                        // wygląda jak zawieszenie.
                        if let Some((_, tool)) = progress.borrow_mut().as_mut() {
                            *tool = cmd.tool_name().to_string();
                        }
                        let res = match handle_drum_command(&cmd, &psync, vm.lang()) {
                            Some(r) => r,
                            None => vm.execute_tool(&cmd),
                        };
                        r.send_tool_result(res);
                        did_work = true;
                    }
                    // Zdarzenia końcowe.
                    while let Some(ev) = r.try_event() {
                        match ev {
                            AgentEvent::Done { history, usage } => {
                                vm.apply_run_outcome(history, usage);
                                done = true;
                            }
                            AgentEvent::Error(e) => {
                                vm.report_error(e);
                                done = true;
                            }
                        }
                    }
                }
            }
            if done {
                *slot.borrow_mut() = None;
                *progress.borrow_mut() = None;
                ui.set_agent_busy(false);
                ui.set_agent_status(SharedString::new());
            } else {
                // Tyka co 40 ms → licznik sekund żyje nawet, gdy agent czeka na model.
                let lang = vmc.borrow().lang();
                ui.set_agent_status(agent_status_text(lang, &progress.borrow()).into());
            }
            // Odśwież tylko po realnej pracy — nie przebudowuj UI 25×/s bez potrzeby.
            if did_work || done {
                refresh(&ui, &mut vmc.borrow_mut());
            }
        });
    }

    // Pompa zrzutu urządzenia (W2) + okresowa detekcja hotplug (W6). Osobny
    // Timer, bo pompa agenta wychodzi wcześnie gdy nie ma przebiegu.
    let device_pump = Timer::default();
    {
        use mg101_desktop::device::DumpMsg;
        use mg101_desktop::vm::SlotRow;
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let dslot = dumper.clone();
        let krx = key_load.clone();
        let pending = pending_run.clone();
        let arunner = runner.clone();
        let aprogress = agent_progress.clone();
        let ldump = last_dump.clone();
        let srecs = slot_records.clone();
        let psync = presync.clone();
        let aslot = active_slot.clone();
        let afetch = auto_fetched.clone();
        let mon_on = midi_mon_on.clone();
        let mlog = midi_log.clone();
        let tick = RefCell::new(0u32);
        device_pump.start(TimerMode::Repeated, Duration::from_millis(100), move || {
            let Some(ui) = uw.upgrade() else { return };
            // Klucz API z Keychain (leniwy odczyt w tle) — wstrzyknij, gdy dotrze,
            // i wystartuj przebieg agenta, jeśli wiadomość czekała na klucz.
            {
                let arrived = match &*krx.borrow() {
                    KeyLoad::Loading(rx) => rx.try_recv().ok(),
                    _ => None,
                };
                if let Some(result) = arrived {
                    *krx.borrow_mut() = KeyLoad::Done; // magazyn odpowiedział — nie pytamy ponownie
                    let mut vm = vmc.borrow_mut();
                    let mut cfg = vm.agent_config().clone();
                    if cfg.api_key.is_empty() {
                        match result {
                            Some(key) => {
                                // NIGDY nie logujemy wartości klucza — tylko dostawca i długość.
                                eprintln!(
                                    "Klucz API wczytany z magazynu (dostawca {:?}, długość {}).",
                                    cfg.provider,
                                    key.len()
                                );
                                cfg.api_key = key;
                                vm.set_agent_config(cfg);
                                ui.set_cfg_key(vm.agent_config().api_key.clone().into());
                            }
                            None => {
                                eprintln!("Brak klucza API w magazynie — wpisz w Ustawieniach.")
                            }
                        }
                    }
                    // Wiadomość odłożona do czasu odczytu klucza.
                    if *pending.borrow() {
                        *pending.borrow_mut() = false;
                        if vm.is_agent_configured() && arunner.borrow().is_none() {
                            let config = vm.agent_config().clone();
                            let history = vm.chat_history();
                            *arunner.borrow_mut() = Some(ChatRunner::start(
                                config,
                                SYSTEM_PROMPT.to_string(),
                                history,
                            ));
                            *aprogress.borrow_mut() =
                                Some((std::time::Instant::now(), String::new()));
                        } else {
                            vm.report_error(
                                "agent nieskonfigurowany (uzupełnij ustawienia AI)".into(),
                            );
                            ui.set_agent_busy(false);
                        }
                    }
                    refresh(&ui, &mut vm);
                }
            }
            // Detekcja hotplug co ~2 s (gdy nie trwa zrzut) + utrzymanie trwałej
            // sesji synchronizacji presetu wraz ze stanem połączenia.
            {
                let mut t = tick.borrow_mut();
                *t = t.wrapping_add(1);
                if t.is_multiple_of(20) && dslot.borrow().is_none() {
                    detect_device(&ui, vmc.borrow().lang());
                    // Ostrzeżenie o równoległym QuickTone (konkurencja o port + zmiany
                    // w QT nie emitują Program Change → nasz sync ich nie widzi).
                    ui.set_qt_running(mg101_desktop::device::quicktone_running());
                    let connected = ui.get_device_connected();
                    if connected && psync.borrow().is_none() {
                        // Otwórz trwałe łącze sync (Program Change w obie strony).
                        if let Some(d) = mg101_desktop::device::detect() {
                            match mg101_desktop::device::PresetSync::start(d.port_needle) {
                                Ok(s) => {
                                    // Urządzenie nie rozgłasza tempa przy starcie — pytamy o nie,
                                    // inaczej DRUM pokazywałby „— BPM" aż do zmiany na sprzęcie.
                                    s.send_raw(mg101_desktop::drum::tempo_request());
                                    *psync.borrow_mut() = Some(s);
                                    ui.set_device_live(true);
                                }
                                Err(e) => {
                                    eprintln!("Sync presetu niedostępny: {e}");
                                    ui.set_device_live(false);
                                }
                            }
                        }
                    } else if !connected && psync.borrow().is_some() {
                        *psync.borrow_mut() = None; // odłączono → zamknij łącze
                        ui.set_device_live(false);
                        *aslot.borrow_mut() = -1;
                        ui.set_active_slot(-1);
                    }
                    if !connected {
                        *afetch.borrow_mut() = false; // reset — kolejne podłączenie znów pobierze
                    }
                    // Pierwszy Fetch automatycznie po wykryciu urządzenia (raz na połączenie).
                    if connected && !*afetch.borrow() && dslot.borrow().is_none() {
                        *afetch.borrow_mut() = true;
                        ui.invoke_connect_dump();
                    }
                }
            }
            // Zdarzenia footswitcha (urządzenie → host): podświetl aktywny preset
            // i — jeśli slot jest już zdekodowany ze zrzutu — otwórz go w edytorze.
            {
                let events = psync
                    .borrow()
                    .as_ref()
                    .map(|s| s.poll())
                    .unwrap_or_default();
                for ev in events {
                    match ev {
                        mg101_desktop::device::SyncEvent::PresetChanged(n) => {
                            let idx = n as i32;
                            if *aslot.borrow() != idx {
                                *aslot.borrow_mut() = idx;
                                ui.set_active_slot(idx);
                                let key = ("user".to_string(), n as u16);
                                let entry = srecs.borrow().get(&key).cloned();
                                if let Some((name, record)) = entry {
                                    let id = format!("device-user-{n}");
                                    vmc.borrow_mut().open_device_patch(&id, &name, record);
                                    refresh(&ui, &mut vmc.borrow_mut());
                                }
                            }
                        }
                        // Centralny EXP zmieniony guzikiem na urządzeniu → odwzoruj w dropdownie.
                        mg101_desktop::device::SyncEvent::ExpTarget(v) => {
                            ui.set_exp_index(v as i32);
                        }
                        // Tempo DRUM zmienione na urządzeniu → pokaż BPM.
                        mg101_desktop::device::SyncEvent::DrumTempo(bpm) => {
                            ui.set_drum_tempo(bpm as i32);
                        }
                    }
                }
            }
            // Monitor MIDI: zawsze drenujemy kanał (by się nie zapchał); gdy monitor
            // włączony — formatujemy i dokładamy do bufora (cap), aktualizujemy widok.
            {
                let raw = psync
                    .borrow()
                    .as_ref()
                    .map(|s| s.poll_monitor())
                    .unwrap_or_default();
                if *mon_on.borrow() && !raw.is_empty() {
                    let mut log = mlog.borrow_mut();
                    for m in &raw {
                        log.push(describe_midi(m));
                    }
                    // Ogranicz bufor (ostatnie 2000 linii) — długi nasłuch nie rośnie bez końca.
                    let len = log.len();
                    if len > 2000 {
                        log.drain(0..len - 2000);
                    }
                    ui.set_midi_log(log.join("\n").into());
                }
            }
            if dslot.borrow().is_none() {
                return;
            }
            let msgs = dslot
                .borrow()
                .as_ref()
                .map(|d| d.poll())
                .unwrap_or_default();
            let mut finished = false;
            for m in msgs {
                match m {
                    DumpMsg::Progress { done, total } => {
                        ui.set_dump_status(format!("pobieranie {done}/{total} slotów…").into());
                    }
                    DumpMsg::Done { user, factory } => {
                        // Zdekoduj zajęte sloty do rekordów plikowych (8402 B) — gotowe
                        // do importu z pełnymi parametrami (kodek wire→plik W2). Zapisz
                        // też per slot (do otwierania pojedynczego slotu w edytorze).
                        let mut import_bytes = Vec::new();
                        let mut recs = srecs.borrow_mut();
                        recs.clear();
                        for s in user.iter().chain(factory.iter()).filter(|s| s.occupied()) {
                            if let Ok(rec) = mg101_pack_nux_mg101::wire::decode_slot(&s.blob) {
                                import_bytes.extend_from_slice(&rec);
                                let name = mg101_pack_nux_mg101::wire::decode_name(&s.blob);
                                recs.insert((s.bank.clone(), s.index), (name, rec));
                            }
                        }
                        drop(recs);
                        *ldump.borrow_mut() = import_bytes;

                        let to_rows = |dump: Vec<mg101_desktop::device::SlotDump>| {
                            dump.into_iter()
                                .map(|s| SlotRow {
                                    index: s.index,
                                    // Nazwa dekodowana z rekordu wire (kodek W2).
                                    name: mg101_pack_nux_mg101::wire::decode_name(&s.blob),
                                    occupied: s.occupied(),
                                    writable: s.bank == "user",
                                })
                                .collect::<Vec<_>>()
                        };
                        let (nu, nf) = (user.len(), factory.len());
                        vmc.borrow_mut()
                            .set_device_banks(to_rows(user), to_rows(factory));
                        ui.set_dump_status(format!("pobrano User {nu} + Factory {nf}").into());
                        ui.set_dump_ready(!ldump.borrow().is_empty());
                        // Na urządzeniu zawsze jest aktywny jeden preset User — po pobraniu
                        // odwzoruj to zaznaczeniem w aplikacji, żeby edytor nie był pusty.
                        // Preferuj znany aktywny preset (footswitch), inaczej pierwszy zajęty
                        // slot User. Bez wysyłki PC (to odwzorowanie, nie komenda).
                        if vmc.borrow().tab() == LibraryTab::User {
                            let want = *aslot.borrow();
                            let pick = if want >= 0 {
                                want as u16
                            } else {
                                srecs
                                    .borrow()
                                    .keys()
                                    .filter(|(b, _)| b == "user")
                                    .map(|(_, i)| *i)
                                    .min()
                                    .unwrap_or(0)
                            };
                            let entry = srecs.borrow().get(&("user".to_string(), pick)).cloned();
                            if let Some((name, record)) = entry {
                                let id = format!("device-user-{pick}");
                                vmc.borrow_mut().open_device_patch(&id, &name, record);
                            }
                        }
                        finished = true;
                    }
                    DumpMsg::Error(e) => {
                        vmc.borrow_mut().report_error(e);
                        ui.set_dump_status("błąd pobierania — patrz komunikat".into());
                        finished = true;
                    }
                }
            }
            if finished {
                *dslot.borrow_mut() = None;
                ui.set_dump_busy(false);
                refresh(&ui, &mut vmc.borrow_mut());
            }
        });
    }

    ui.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{device_user_index, slot_label};

    #[test]
    fn device_user_index_parses_only_user_device_ids() {
        assert_eq!(device_user_index("device-user-0"), Some(0));
        assert_eq!(device_user_index("device-user-35"), Some(35));
        assert_eq!(device_user_index("device-factory-3"), None);
        assert_eq!(device_user_index("import-deadbeef"), None);
        assert_eq!(device_user_index("device-user-"), None);
    }

    #[test]
    fn slot_label_maps_index_to_footswitch_1a_9d() {
        // 9 banków × 4 litery (A–D): 0→1A, 1→1B, 4→2A, 35→9D.
        assert_eq!(slot_label(0), "1A");
        assert_eq!(slot_label(1), "1B");
        assert_eq!(slot_label(3), "1D");
        assert_eq!(slot_label(4), "2A");
        assert_eq!(slot_label(35), "9D");
    }

    #[test]
    fn slot_label_out_of_range_falls_back_to_raw_number() {
        assert_eq!(slot_label(36), "36");
        assert_eq!(slot_label(100), "100");
    }
}

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

/// Katalog obrazów modeli (poza repo — IP; wypełniany `tools/extract_quicktone_images.py`).
fn model_images_dir() -> std::path::PathBuf {
    app_data_dir().join("model-images")
}

/// Ładuje grafikę modelu `<blok>_<model_id>.png` z lokalnego katalogu, jeśli jest.
/// Brak pliku to normalny przypadek (efekty bez zdjęcia, brak ekstrakcji) → `None`.
fn model_image(block: &str, model_id: i64) -> Option<slint::Image> {
    let path = model_images_dir().join(format!("{block}_{model_id}.png"));
    if !path.exists() {
        return None;
    }
    slint::Image::load_from_path(&path).ok()
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
                })
                .collect();
            let opts = vm.models(&b.block);
            let names: Vec<SharedString> = opts.iter().map(|o| o.name.clone().into()).collect();
            let idx = opts.iter().position(|o| o.id == b.model_id).unwrap_or(0) as i32;
            // Grafika modelu (W4): plik lokalny `<moduł>_<model_id>.png` (poza repo).
            let (image, has_image) = match model_image(&b.block, b.model_id) {
                Some(img) => (img, true),
                None => (slint::Image::default(), false),
            };
            BlockRowUi {
                block: b.block.clone().into(),
                model_name: b.model_name.clone().into(),
                bypassed: b.bypassed,
                params: ModelRc::new(VecModel::from(params)),
                models: ModelRc::new(VecModel::from(names)),
                model_index: idx,
                image,
                has_image,
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
    ui.set_t_empty_slot(l("slot.empty"));
    ui.set_t_rev(l("editor.rev"));
    ui.set_t_settings(l("inspector.settings"));
    ui.set_t_provider(l("settings.provider"));
    ui.set_t_endpoint(l("settings.endpoint"));
    ui.set_t_model(l("settings.model"));
    ui.set_t_key(l("settings.key"));
    ui.set_t_save(l("action.save"));
    ui.set_t_send(l("agent.send"));
    ui.set_lang_index(if vm.lang() == Lang::Pl { 1 } else { 0 });
}

/// Odświeża dane na oknie z bieżącego stanu VM (nie dotyka `agent-busy`).
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

    let slots: Vec<SlotRowUi> = vm
        .slot_rows()
        .iter()
        .map(|s| SlotRowUi {
            index: s.index as i32,
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
fn detect_device(ui: &AppWindow) {
    match mg101_desktop::device::detect() {
        Some(d) => {
            ui.set_device_label(format!("{} {}", d.manufacturer, d.model).into());
            ui.set_device_connected(true);
        }
        None => {
            ui.set_device_label("brak urządzenia".into());
            ui.set_device_connected(false);
        }
    }
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

    // Klucz API z systemowego magazynu (parytet v1 KeychainStore) — ODCZYT W TLE.
    // Na macOS dostęp do Keychain potrafi zablokować wątek do czasu zgody w oknie
    // systemowym; robienie tego na wątku UI przed `run()` zawieszało start okna.
    // Odbiornik odpytujemy w pompie i wstrzykujemy klucz, gdy dotrze.
    let key_provider = vm.borrow().agent_config().provider;
    let key_rx: Rc<RefCell<Option<std::sync::mpsc::Receiver<Option<String>>>>> = Rc::new(
        RefCell::new(Some(mg101_desktop::keychain::load_key_async(key_provider))),
    );

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
    detect_device(&ui);
    // Uchwyt zrzutu w tle (W2) — Some tylko podczas trwającego zrzutu.
    let dumper: Rc<RefCell<Option<mg101_desktop::device::Dumper>>> = Rc::new(RefCell::new(None));
    // Ostatni zrzut zdekodowany do rekordów plikowych (8402 B × zajęte sloty),
    // gotowy do importu do Biblioteki. Pusty, dopóki nie ma zrzutu.
    let last_dump: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    // Zdekodowane rekordy per slot (bank, index) → (nazwa, bajty plikowe) — do
    // otwierania pojedynczego slotu w edytorze po kliknięciu.
    let slot_records: SlotRecords = Rc::new(RefCell::new(std::collections::HashMap::new()));

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
    wire!(on_set_param, |vm, id, rev, block, param, value| {
        vm.set_parameter(&id, rev as i64, &block, &param, value.round() as i64);
    });
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
        ui.on_open_slot(move |bank, index| {
            let Some(ui) = uw.upgrade() else { return };
            let key = (bank.to_string(), index as u16);
            let entry = srecs.borrow().get(&key).cloned();
            if let Some((name, record)) = entry {
                let id = format!("device-{bank}-{index}");
                vmc.borrow_mut().open_device_patch(&id, &name, record);
            } else {
                vmc.borrow_mut()
                    .report_error("brak zdekodowanego slotu — wykonaj zrzut".into());
            }
            refresh(&ui, &mut vmc.borrow_mut());
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

    // Wysłanie wiadomości do agenta: startuje wątek przebiegu.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let slot = runner.clone();
        ui.on_send_message(move |text| {
            let Some(ui) = uw.upgrade() else { return };
            let mut vm = vmc.borrow_mut();
            if slot.borrow().is_some() {
                return; // przebieg już trwa
            }
            if !vm.is_agent_configured() {
                vm.report_error("agent nieskonfigurowany (uzupełnij ustawienia AI)".into());
            } else {
                vm.push_user_message(&text);
                let config = vm.agent_config().clone();
                let history = vm.chat_history();
                *slot.borrow_mut() = Some(ChatRunner::start(
                    config,
                    SYSTEM_PROMPT.to_string(),
                    history,
                ));
                ui.set_agent_busy(true);
            }
            refresh(&ui, &mut vm);
        });
    }

    // Pompa wątku agenta: obsługuje żądania narzędzi (na wątku UI, na Studio) i
    // zdarzenia końcowe. Timer musi żyć do końca `run()`.
    let pump = Timer::default();
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        let slot = runner.clone();
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
                        let res = vm.execute_tool(&cmd);
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
                ui.set_agent_busy(false);
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
        let krx = key_rx.clone();
        let ldump = last_dump.clone();
        let srecs = slot_records.clone();
        let tick = RefCell::new(0u32);
        device_pump.start(TimerMode::Repeated, Duration::from_millis(100), move || {
            let Some(ui) = uw.upgrade() else { return };
            // Klucz API z Keychain (odczyt w tle) — wstrzyknij, gdy dotrze.
            {
                let mut slot = krx.borrow_mut();
                if let Some(rx) = slot.as_ref() {
                    if let Ok(result) = rx.try_recv() {
                        *slot = None; // jednorazowo
                        let mut vm = vmc.borrow_mut();
                        let mut cfg = vm.agent_config().clone();
                        if cfg.api_key.is_empty() {
                            match result {
                                Some(key) => {
                                    eprintln!(
                                        "Klucz API wczytany z magazynu (dostawca {:?}, długość {}).",
                                        cfg.provider,
                                        key.len()
                                    );
                                    cfg.api_key = key;
                                    vm.set_agent_config(cfg);
                                    ui.set_cfg_key(vm.agent_config().api_key.clone().into());
                                }
                                None => eprintln!("Brak klucza API w magazynie — wpisz w Ustawieniach."),
                            }
                        }
                    }
                }
            }
            // Detekcja hotplug co ~2 s (gdy nie trwa zrzut).
            {
                let mut t = tick.borrow_mut();
                *t = t.wrapping_add(1);
                if t.is_multiple_of(20) && dslot.borrow().is_none() {
                    detect_device(&ui);
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
                        ui.set_dump_status(format!("zrzut {done}/{total} slotów…").into());
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
                        ui.set_dump_status(format!("zrzucono User {nu} + Factory {nf}").into());
                        ui.set_dump_ready(!ldump.borrow().is_empty());
                        finished = true;
                    }
                    DumpMsg::Error(e) => {
                        vmc.borrow_mut().report_error(e);
                        ui.set_dump_status("błąd zrzutu — patrz komunikat".into());
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

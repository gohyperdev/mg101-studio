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
use mg101_library::MemoryStore;
use mg101_studio::Studio;
use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

type Vm = ViewModel<MemoryStore>;

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
                    value: p.value as i32,
                    minimum: p.minimum as i32,
                    maximum: p.maximum as i32,
                })
                .collect();
            let opts = vm.models(&b.block);
            let names: Vec<SharedString> = opts.iter().map(|o| o.name.clone().into()).collect();
            let idx = opts.iter().position(|o| o.id == b.model_id).unwrap_or(0) as i32;
            BlockRowUi {
                block: b.block.clone().into(),
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
        }
        None => {
            ui.set_has_selection(false);
            ui.set_current_id(SharedString::new());
            ui.set_detail(empty_detail());
            ui.set_changes_text(SharedString::new());
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (profile, catalog) = mg101_pack_nux_mg101::load()?;
    let profile = Box::leak(Box::new(profile));
    let catalog = Box::leak(Box::new(catalog));
    let studio = Studio::new(MemoryStore::new(), profile, catalog, 0)
        .with_journal(Box::new(SessionJournal::default()));
    let vm: Rc<RefCell<Vm>> = Rc::new(RefCell::new(ViewModel::new(studio, Lang::En)));
    let runner: Rc<RefCell<Option<ChatRunner>>> = Rc::new(RefCell::new(None));

    let ui = AppWindow::new()?;
    apply_labels(&ui, &vm.borrow());
    refresh(&ui, &mut vm.borrow_mut());

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
    });

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

    ui.run()?;
    Ok(())
}

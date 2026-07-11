//! Binarka desktop MG101 Studio — okno Slint spięte z [`ViewModel`].
//!
//! UI (Slint) jest cienką powłoką: renderuje struktury z VM i woła jego metody.
//! Cała logika i stan są w [`mg101_desktop::vm`] (te same komendy co agent/MCP).

use std::cell::RefCell;
use std::rc::Rc;

use mg101_core::wal::{JournalStore, TransactionEntry, WalError};
use mg101_desktop::vm::LibraryTab;
use mg101_desktop::{Lang, ViewModel};
use mg101_library::MemoryStore;
use mg101_studio::Studio;
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

type Vm = ViewModel<MemoryStore>;

/// Dziennik WAL sesji w pamięci — daje działający revert (review E7/K2). Stan
/// crash-recovery jest efemeryczny (jak ustalono w BACKLOG E3), więc pamięciowy
/// dziennik na czas życia procesu jest wystarczający dla undo w UI.
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

fn detail_to_ui(d: &mg101_desktop::PatchDetail) -> DetailUi {
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
            BlockRowUi {
                block: b.block.clone().into(),
                model_name: b.model_name.clone().into(),
                bypassed: b.bypassed,
                params: ModelRc::new(VecModel::from(params)),
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
    ui.set_t_language(l("settings.language"));
    ui.set_t_empty_slot(l("slot.empty"));
    ui.set_t_rev(l("editor.rev"));
    ui.set_lang_index(if vm.lang() == Lang::Pl { 1 } else { 0 });
}

/// Odświeża dane na oknie z bieżącego stanu VM.
fn refresh(ui: &AppWindow, vm: &mut Vm) {
    ui.set_active_tab(tab_index(vm.tab()));

    // lista biblioteki
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

    // sloty urządzenia (zakładki User/Factory)
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

    // zaznaczenie + edytor
    match vm.selected_id() {
        Some(id) => {
            ui.set_has_selection(true);
            ui.set_current_id(id.clone().into());
            match vm.detail(&id) {
                Some(d) => ui.set_detail(detail_to_ui(&d)),
                None => ui.set_detail(empty_detail()),
            }
            // inspektor Changes
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

    ui.set_error_text(vm.last_error().unwrap_or("").into());
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (profile, catalog) = mg101_pack_nux_mg101::load()?;
    let profile = Box::leak(Box::new(profile));
    let catalog = Box::leak(Box::new(catalog));
    let studio = Studio::new(MemoryStore::new(), profile, catalog, 0)
        .with_journal(Box::new(SessionJournal::default()));
    let vm: Rc<RefCell<Vm>> = Rc::new(RefCell::new(ViewModel::new(studio, Lang::En)));

    let ui = AppWindow::new()?;
    apply_labels(&ui, &vm.borrow());
    refresh(&ui, &mut vm.borrow_mut());

    // Makro spinające callback z VM: pożycza VM, wykonuje, odświeża okno.
    macro_rules! wire {
        ($setter:ident, |$vm:ident $(, $arg:ident)*| $body:block) => {{
            let uw = ui.as_weak();
            let vmc = vm.clone();
            ui.$setter(move |$($arg),*| {
                let ui = uw.unwrap();
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
    wire!(on_import, |vm| {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("MG-101 patch", &["mg101patch"])
            .pick_file()
        {
            vm.import(&path.to_string_lossy());
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

    // Zmiana języka: przelicz etykiety + odśwież.
    {
        let uw = ui.as_weak();
        let vmc = vm.clone();
        ui.on_switch_lang(move |idx| {
            let ui = uw.unwrap();
            let mut vm = vmc.borrow_mut();
            vm.set_lang(if idx == 1 { Lang::Pl } else { Lang::En });
            apply_labels(&ui, &vm);
            refresh(&ui, &mut vm);
        });
    }

    ui.run()?;
    Ok(())
}

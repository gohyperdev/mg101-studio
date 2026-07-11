//! Binarka desktop MG101 Studio.
//!
//! E7 (bieżący etap): spina profil/katalog + skład w [`ViewModel`] i wypisuje
//! stan startowy. Warstwa Slint (okno) dochodzi w kolejnym kroku E7 — logika UI
//! (VM/i18n) jest już kompletna i testowana cross-platform.

use mg101_desktop::{Lang, LibraryTab, ViewModel};
use mg101_library::MemoryStore;
use mg101_studio::Studio;

fn main() {
    let (profile, catalog) = match mg101_pack_nux_mg101::load() {
        Ok(pc) => pc,
        Err(e) => {
            eprintln!("Nie udało się wczytać profilu MG-101: {e}");
            std::process::exit(1);
        }
    };
    let profile = Box::leak(Box::new(profile));
    let catalog = Box::leak(Box::new(catalog));

    let studio = Studio::new(MemoryStore::new(), profile, catalog, 0);
    let mut vm = ViewModel::new(studio, Lang::En);
    vm.set_tab(LibraryTab::Library);

    println!("{} — {}", vm.label("app.title"), vm.label("library.title"));
    println!("Patchy w bibliotece: {}", vm.rows().len());
    println!("(UI Slint dochodzi w kolejnym kroku E7; logika VM/i18n gotowa i testowana.)");
}

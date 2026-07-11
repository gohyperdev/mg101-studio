//! Kompilacja UI Slint do kodu Rust (natywne cele). Na wasm pomijana — UI Slint
//! nie wchodzi do builda wasm (rdzeń ma być tylko „web-ready", E8).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }
    slint_build::compile("ui/app.slint").expect("kompilacja app.slint");
}

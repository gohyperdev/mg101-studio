//! Dowód #7 (warunek celu): kodek round-trip 1:1 wykonany w **WASM**.
//! Oracle wbudowany (`include_bytes!`), więc nie potrzeba dostępu do FS.
//!
//! Build:  cargo build -p mg101-pack-nux-mg101 --example wasm_roundtrip \
//!            --target wasm32-wasip1
//! Uruchom: wasmtime target/wasm32-wasip1/debug/examples/wasm_roundtrip.wasm

use mg101_core::{canonical::CanonicalPatch, PatchRecord};
use mg101_pack_nux_mg101::{load, Container};

const ORACLE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../Sources/MG101Core/Resources/factory-patches.mg101patch"
));

fn main() {
    let (profile, _catalog) = load().expect("profil MG-101");
    let records = Container::split(ORACLE, profile.record_size).expect("split");
    let total = records.len();
    let mut mismatches = 0usize;
    for chunk in &records {
        let record = PatchRecord::new(chunk.to_vec(), &profile).expect("rozmiar OK");
        let reencoded = CanonicalPatch::decode(&record).encode(&profile);
        if &reencoded[..] != *chunk {
            mismatches += 1;
        }
    }
    // target_arch dowodzi, że kod wykonuje się faktycznie w wasm.
    println!(
        "[WASM arch={}] round-trip: {}/{} rekordów, różnic: {}",
        std::env::consts::ARCH,
        total - mismatches,
        total,
        mismatches
    );
    assert_eq!(mismatches, 0, "round-trip 1:1 w WASM");
    println!("WYNIK WASM: 0 różnic — kodek round-trip 1:1 POTWIERDZONY w wasm.");
}

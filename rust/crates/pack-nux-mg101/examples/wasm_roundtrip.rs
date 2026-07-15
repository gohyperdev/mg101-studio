//! Dowód (warunek celu): kodek round-trip 1:1 wykonany w **WASM**.
//! Dane to syntetyczny seed generowany z profilu (bez własności producenta),
//! więc nie potrzeba dostępu do FS ani plików fabrycznych.
//!
//! Build:  cargo build -p mg101-pack-nux-mg101 --example wasm_roundtrip \
//!            --target wasm32-wasip1
//! Uruchom: wasmtime target/wasm32-wasip1/debug/examples/wasm_roundtrip.wasm

use mg101_core::{canonical::CanonicalPatch, PatchRecord};
use mg101_pack_nux_mg101::{load, seed_patches, Container};

fn main() {
    let (profile, catalog) = load().expect("profil MG-101");
    let data = seed_patches(&profile, &catalog);
    let records = Container::split(&data, profile.record_size).expect("split");
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

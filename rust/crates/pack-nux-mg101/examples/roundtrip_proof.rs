//! Dowód #3 (warunek celu): round-trip 1:1 bajt-w-bajt na REALNYM pliku
//! `.mg101patch` (oracle Swifta). Czyta plik w runtime, dekoduje każdy rekord do
//! modelu kanonicznego, re-enkoduje i porównuje bajt-w-bajt. Raportuje liczbę
//! rekordów i różnice.
//!
//! Uruchom: `cargo run -p mg101-pack-nux-mg101 --example roundtrip_proof`

use mg101_core::{canonical::CanonicalPatch, PatchRecord};
use mg101_pack_nux_mg101::{load, Container};

fn main() {
    let oracle_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../Sources/MG101Core/Resources/factory-patches.mg101patch"
    );
    let data = std::fs::read(oracle_path).expect("odczyt oracle .mg101patch");
    let (profile, _catalog) = load().expect("profil MG-101");

    println!("Oracle: {oracle_path}");
    println!(
        "Rozmiar pliku: {} B, rozmiar rekordu: {} B",
        data.len(),
        profile.record_size
    );

    let records = Container::split(&data, profile.record_size).expect("split na rekordy");
    let total = records.len();
    let mut mismatches = 0usize;
    let mut mismatch_bytes = 0usize;

    for (i, chunk) in records.iter().enumerate() {
        let record = PatchRecord::new(chunk.to_vec(), &profile).expect("rozmiar rekordu OK");
        let canonical = CanonicalPatch::decode(&record);
        let reencoded = canonical.encode(&profile);
        if &reencoded[..] != *chunk {
            mismatches += 1;
            let diff = reencoded
                .iter()
                .zip(chunk.iter())
                .filter(|(a, b)| a != b)
                .count();
            mismatch_bytes += diff;
            eprintln!("  rekord {i}: {diff} różniących się bajtów");
        }
    }

    println!(
        "Round-trip decode→encode: {} / {total} rekordów, różnic bajtowych: {}",
        total - mismatches,
        mismatch_bytes
    );
    if mismatches == 0 {
        println!("WYNIK: 0 różnic — round-trip 1:1 bajt-w-bajt POTWIERDZONY.");
    } else {
        eprintln!("WYNIK: {mismatches} rekordów z różnicami — NIEPOWODZENIE.");
        std::process::exit(1);
    }
}

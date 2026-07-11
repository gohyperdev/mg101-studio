//! Wykonanie komend na PLIKACH (port `executeFileCommand`/`inspect` z MCP v1).
//!
//! Ścieżka plikowa (cel `File{input, output}`): wczytaj rekord, zmutuj przez
//! `PatchRecord` (sterowane profilem/katalogiem — DANE), zapisz output. Osobna od
//! ścieżki bibliotecznej (rewizje/WAL) — używana przez MCP stdio. Device-agnostyczna.
//!
//! Natywna (FS); na wasm niedostępna.

#![cfg(not(target_arch = "wasm32"))]

use crate::ExecError;
use mg101_commands::{Command, TargetRef};
use mg101_core::{DeviceProfile, EffectCatalog, PatchRecord};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

fn file_target(target: &TargetRef) -> Result<(&str, &str), ExecError> {
    match target {
        TargetRef::File { input, output } => Ok((input.as_str(), output.as_str())),
        TargetRef::Library { .. } => Err(ExecError::Unsupported(
            "ścieżka plikowa wymaga celu File{input,output}".into(),
        )),
    }
}

fn load_record<'p>(path: &str, profile: &'p DeviceProfile) -> Result<PatchRecord<'p>, ExecError> {
    let data =
        std::fs::read(path).map_err(|e| ExecError::Unsupported(format!("odczyt '{path}': {e}")))?;
    Ok(PatchRecord::new(data, profile)?)
}

/// Zapisuje rekord do `output` **atomowo** i **bez nadpisania** (parytet
/// `PatchFileWriter.writeNew` z v1; naprawa review E6.3/K1).
///
/// Wcześniejsze `exists()` + `fs::write` miało wyścig TOCTOU (równoległe
/// wywołania MCP mogły po cichu nadpisać cudzy patch — naruszenie "bajtów
/// świętych") i zostawiało obcięty plik przy awarii w trakcie zapisu. Teraz:
/// pełny zapis do pliku tymczasowego w katalogu docelowym (+`sync_all`), po czym
/// `hard_link` na ścieżkę docelową — link **atomowo zawodzi, gdy cel istnieje**,
/// więc kontrakt „nie nadpisuj" jest egzekwowany bez okna wyścigu, a plik
/// docelowy nigdy nie jest widoczny w stanie częściowym. Plik wejściowy nie jest
/// dotykany.
fn write_record(record: &PatchRecord, output: &str) -> Result<(), ExecError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let out_path = std::path::Path::new(output);
    // Wczesne odrzucenie istniejącego celu (jasny komunikat); ostateczny gwarant
    // to i tak atomowy hard_link poniżej.
    if out_path.exists() {
        return Err(ExecError::Unsupported(format!(
            "plik wyjściowy już istnieje: {output}"
        )));
    }
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = out_path.with_extension(format!("mg101patch.tmp.{}.{seq}", std::process::id()));

    let write_tmp = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(record.data())?;
        f.sync_all()
    };
    if let Err(e) = write_tmp() {
        let _ = std::fs::remove_file(&tmp);
        return Err(ExecError::Unsupported(format!("zapis '{output}': {e}")));
    }

    // Atomowe „utwórz, jeśli nie istnieje": hard_link zawodzi z AlreadyExists,
    // gdy cel powstał w międzyczasie (domknięcie TOCTOU).
    let link_res = std::fs::hard_link(&tmp, out_path);
    let _ = std::fs::remove_file(&tmp);
    link_res.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            ExecError::Unsupported(format!("plik wyjściowy już istnieje: {output}"))
        } else {
            ExecError::Unsupported(format!("zapis '{output}': {e}"))
        }
    })
}

/// Wykonuje komendę plikową i zwraca komunikat tekstowy (jak MCP v1).
pub fn execute_file_command(
    profile: &DeviceProfile,
    catalog: &EffectCatalog,
    command: &Command,
) -> Result<String, ExecError> {
    match command {
        Command::SetParameter {
            target,
            block,
            parameter,
            value,
        } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            let blk = profile.block(block)?;
            let model_id = rec.model_id(blk);
            let model = catalog.model(block, model_id).ok_or_else(|| {
                ExecError::Unsupported(format!("nieznany model {block}.{model_id}"))
            })?;
            let param = model
                .parameters
                .iter()
                .find(|p| &p.name == parameter)
                .ok_or_else(|| {
                    ExecError::Unsupported(format!("nieznany parametr {block}.{parameter}"))
                })?;
            rec.set_parameter(param, *value)?;
            write_record(&rec, output)?;
            Ok(format!("Zapisano {block}.{parameter}={value} do {output}"))
        }
        Command::SetModel {
            target,
            block,
            model,
            values,
            bypassed,
        } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            let blk = profile.block(block)?;
            let m = catalog
                .model(block, *model)
                .ok_or_else(|| ExecError::Unsupported(format!("nieznany model {block}.{model}")))?;
            rec.set_model(m, blk, values, *bypassed, true)?;
            write_record(&rec, output)?;
            Ok(format!("Zapisano model {block}={model} do {output}"))
        }
        Command::SetBypass {
            target,
            block,
            bypassed,
        } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            let blk = profile.block(block)?;
            rec.set_bypass(*bypassed, blk);
            write_record(&rec, output)?;
            Ok(format!("Zapisano bypass {block}={bypassed} do {output}"))
        }
        Command::SetName { target, name } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            rec.set_name(name)?;
            write_record(&rec, output)?;
            Ok(format!("Zapisano nazwę do {output}"))
        }
        Command::SetBpm { target, bpm } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            rec.set_bpm(*bpm)?;
            write_record(&rec, output)?;
            Ok(format!("Zapisano BPM do {output}"))
        }
        Command::SetNamedField {
            target,
            field,
            value,
        } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            rec.set_named_field(field, *value)?;
            write_record(&rec, output)?;
            Ok(format!("Zapisano {field} do {output}"))
        }
        Command::SetIr {
            target,
            wav_path,
            name,
        } => {
            let (input, output) = file_target(target)?;
            let wav = std::fs::read(wav_path)
                .map_err(|e| ExecError::Unsupported(format!("odczyt WAV '{wav_path}': {e}")))?;
            let mut rec = load_record(input, profile)?;
            rec.set_ir(&wav, name)?;
            write_record(&rec, output)?;
            Ok(format!("Osadzono IR w {output}"))
        }
        Command::ClearIr { target } => {
            let (input, output) = file_target(target)?;
            let mut rec = load_record(input, profile)?;
            rec.clear_ir();
            write_record(&rec, output)?;
            Ok(format!("Usunięto IR w {output}"))
        }
        other => Err(ExecError::Unsupported(format!(
            "komenda {} nieobsługiwana w trybie plikowym",
            other.tool_name()
        ))),
    }
}

/// Inspekcja pliku patcha → JSON (port `inspect`).
pub fn inspect(
    profile: &DeviceProfile,
    catalog: &EffectCatalog,
    path: &str,
) -> Result<Value, ExecError> {
    let rec = load_record(path, profile)?;
    let blocks: Vec<Value> = profile
        .blocks
        .iter()
        .map(|block| {
            let model_id = rec.model_id(block);
            json!({
                "block": block.id,
                "model_id": model_id,
                "model": catalog.model(&block.id, model_id).map(|m| m.display_name.clone()).unwrap_or_else(|| "unknown".into()),
                "bypassed": rec.is_bypassed(block),
            })
        })
        .collect();
    let named_fields: serde_json::Map<String, Value> = profile
        .named_fields
        .iter()
        .map(|(k, f)| (k.clone(), json!(rec.value_at(f.offset))))
        .collect();
    Ok(json!({
        "path": path,
        "size": rec.data().len(),
        "slot": rec.slot_index(),
        "name": rec.name(),
        "bpm": rec.bpm(),
        "ir": {"present": rec.ir_present(), "name": rec.ir_name()},
        "named_fields": named_fields,
        "blocks": blocks,
    }))
}

/// Modele bloku → JSON (port `list_models`; kształt 1:1 ze Studio).
pub fn list_models_json(catalog: &EffectCatalog, block: &str) -> Value {
    let list: Vec<Value> = catalog
        .models(block)
        .iter()
        .map(|model| {
            json!({
                "modelID": model.model_id,
                "displayName": model.display_name,
                "parameters": model.parameters.iter().map(|p| json!({
                    "name": p.name,
                    "minimum": p.minimum(),
                    "maximum": p.maximum(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Value::Array(list)
}

/// Profil urządzenia → JSON (port `get_profile`; kształt 1:1 ze Studio).
pub fn profile_json(profile: &DeviceProfile) -> Value {
    let blocks: Vec<Value> = profile
        .blocks
        .iter()
        .map(|b| json!({"id": b.id, "displayName": b.display_name}))
        .collect();
    let named_fields: serde_json::Map<String, Value> = profile
        .named_fields
        .iter()
        .map(|(k, f)| {
            (
                k.clone(),
                json!({"minimum": f.minimum, "maximum": f.maximum}),
            )
        })
        .collect();
    json!({
        "recordSize": profile.record_size,
        "blocks": blocks,
        "namedFields": named_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DeviceProfile, EffectCatalog) {
        mg101_pack_nux_mg101::load().unwrap()
    }

    // Katalog unikalny per test (izolacja — testy biegną równolegle w jednym
    // procesie; wspólny katalog + remove_dir_all ścigałyby się).
    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mg101_fileops_{}_{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn set_bpm_on_file_writes_output_and_inspect_reads_it() {
        let (profile, catalog) = fixture();
        let dir = tmpdir("setbpm");
        let input = dir.join("in.mg101patch");
        let output = dir.join("out.mg101patch");
        let _ = std::fs::remove_file(&output);
        std::fs::write(&input, vec![0u8; profile.record_size]).unwrap();

        let cmd = Command::SetBpm {
            target: TargetRef::File {
                input: input.to_str().unwrap().into(),
                output: output.to_str().unwrap().into(),
            },
            bpm: 120,
        };
        let msg = execute_file_command(&profile, &catalog, &cmd).unwrap();
        assert!(msg.contains("BPM"));

        let v = inspect(&profile, &catalog, output.to_str().unwrap()).unwrap();
        assert_eq!(v["bpm"], 120);
        assert_eq!(v["size"], profile.record_size);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_to_overwrite_existing_output() {
        let (profile, catalog) = fixture();
        let dir = tmpdir("overwrite");
        let input = dir.join("in2.mg101patch");
        let output = dir.join("exists.mg101patch");
        std::fs::write(&input, vec![0u8; profile.record_size]).unwrap();
        std::fs::write(&output, b"zajete").unwrap();
        let cmd = Command::SetName {
            target: TargetRef::File {
                input: input.to_str().unwrap().into(),
                output: output.to_str().unwrap().into(),
            },
            name: "X".into(),
        };
        let err = execute_file_command(&profile, &catalog, &cmd).unwrap_err();
        assert!(matches!(err, ExecError::Unsupported(m) if m.contains("już istnieje")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_tmp_and_preserves_existing_output() {
        let (profile, catalog) = fixture();
        let dir = tmpdir("atomic");
        let input = dir.join("in3.mg101patch");
        let output = dir.join("out3.mg101patch");
        let _ = std::fs::remove_file(&output);
        std::fs::write(&input, vec![0u8; profile.record_size]).unwrap();
        let input_before = std::fs::read(&input).unwrap();

        let cmd = |bpm: i64| Command::SetBpm {
            target: TargetRef::File {
                input: input.to_str().unwrap().into(),
                output: output.to_str().unwrap().into(),
            },
            bpm,
        };
        // Pierwszy zapis tworzy output; plik wejściowy nietknięty.
        execute_file_command(&profile, &catalog, &cmd(77)).unwrap();
        assert_eq!(
            std::fs::read(&input).unwrap(),
            input_before,
            "input dotknięty"
        );
        let output_after_first = std::fs::read(&output).unwrap();

        // Drugi zapis na istniejący output jest odrzucony — istniejący plik bez zmian.
        assert!(execute_file_command(&profile, &catalog, &cmd(11)).is_err());
        assert_eq!(
            std::fs::read(&output).unwrap(),
            output_after_first,
            "odrzucony zapis zmienił istniejący output"
        );

        // Żaden plik tymczasowy nie został (sukces i porażka po sobie sprzątają).
        let leftover: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(
            leftover.is_empty(),
            "wyciek pliku tymczasowego: {leftover:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_target_rejected_in_file_mode() {
        let (profile, catalog) = fixture();
        let cmd = Command::SetBpm {
            target: TargetRef::Library {
                patch_id: "p".into(),
                expected_revision: 1,
            },
            bpm: 100,
        };
        assert!(execute_file_command(&profile, &catalog, &cmd).is_err());
    }
}

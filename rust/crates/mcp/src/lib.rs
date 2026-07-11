//! Serwer MCP (rmcp) po stdio — port `Sources/MG101MCP/main.swift`.
//!
//! Wystawia narzędzia **plikowe** (wariant `File` rejestru komend E3): każda
//! komenda mutująca wczytuje `input`, edytuje przez rdzeń i zapisuje `output`
//! (executor: [`mg101_studio::file_ops`]). Dodatkowo `inspect_patch` (plik→JSON),
//! `list_models` i `get_profile`. Narzędzia i ich anotacje (`readOnlyHint`/
//! `destructiveHint`/`openWorldHint`) wywodzą się z klasy [`Kind`] — jedno źródło
//! zachowania dla UI, agenta i MCP (ADR-0002).
//!
//! Rdzeń dyspozytora ([`dispatch`], [`tool_list`]) jest czysty i testowalny bez
//! sieci/async; powłoka rmcp (`server`) tylko go opakowuje. Natywny (stdio).

#![cfg(not(target_arch = "wasm32"))]

use mg101_commands::{Command, Kind, ToolDefinition, Variant};
use mg101_core::{DeviceProfile, EffectCatalog};
use mg101_studio::file_ops;
use serde_json::{Map, Value};

pub mod server;

/// Narzędzia wystawiane wyłącznie przez MCP (poza rejestrem wariantu File):
/// inspekcja pliku patcha.
const INSPECT_PATCH: &str = "inspect_patch";

/// Narzędzia wariantu File, które faktycznie działają na plikach/rdzeniu MCP.
/// Pomija narzędzia biblioteczne (wymagają składu z rewizjami — brak w trybie
/// plikowym): `list_patches`/`get_patch`/`get_selection`/`get_diff`.
fn is_mcp_tool(name: &str) -> bool {
    matches!(
        name,
        "set_parameter"
            | "set_model"
            | "set_bypass"
            | "set_name"
            | "set_bpm"
            | "set_named_field"
            | "set_ir"
            | "clear_ir"
            | "list_models"
            | "get_profile"
    )
}

/// Opis narzędzia MCP: nazwa, opis, schemat wejścia (JSON Schema jako obiekt),
/// klasa (→ anotacje).
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Map<String, Value>,
    pub kind: Kind,
}

fn as_object(schema: Value) -> Map<String, Value> {
    match schema {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Pełna lista narzędzi serwera MCP (port `allDefinitions(variant:.file)` +
/// `inspect_patch`).
pub fn tool_list() -> Vec<McpTool> {
    let mut tools: Vec<McpTool> = ToolDefinition::all(Variant::File)
        .into_iter()
        .filter(|d| is_mcp_tool(&d.name))
        .map(|d| McpTool {
            name: d.name,
            description: d.description,
            input_schema: as_object(d.input_schema),
            kind: d.kind,
        })
        .collect();

    // inspect_patch: odczyt pliku patcha → JSON (bloki, nazwa, bpm, ir, pola).
    tools.push(McpTool {
        name: INSPECT_PATCH.into(),
        description:
            "Wczytuje plik .mg101patch i zwraca jego zawartość (bloki, modele, nazwa, BPM, IR, pola nazwane)."
                .into(),
        input_schema: as_object(serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string", "description": "Ścieżka bezwzględna do pliku .mg101patch." } },
            "required": ["path"],
            "additionalProperties": false,
        })),
        kind: Kind::Read,
    });
    tools
}

/// Błąd dyspozytora MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// Argument brakujący lub błędnego typu (np. `path` nie-string).
    BadArgument(String),
    /// Wykonanie komendy się nie powiodło (I/O, walidacja rdzenia).
    Execution(String),
    /// Nieznane narzędzie.
    UnknownTool(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::BadArgument(m) => write!(f, "Zły argument: {m}"),
            McpError::Execution(m) => write!(f, "Błąd wykonania: {m}"),
            McpError::UnknownTool(n) => write!(f, "Nieznane narzędzie: {n}"),
        }
    }
}

impl std::error::Error for McpError {}

fn str_arg<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, McpError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::BadArgument(format!("wymagany string '{key}'")))
}

/// Wykonuje narzędzie MCP i zwraca **tekst** wyniku (JSON dla odczytów, komunikat
/// dla mutacji) — port `callTool`/`executeFileCommand`.
///
/// Odczyty (`inspect_patch`/`list_models`/`get_profile`) nie wymagają pliku
/// docelowego; mutacje wymagają `input`+`output` (parsowane przez rejestr) i
/// odmawiają nadpisania istniejącego `output`.
pub fn dispatch(
    profile: &DeviceProfile,
    catalog: &EffectCatalog,
    name: &str,
    args: &Map<String, Value>,
) -> Result<String, McpError> {
    match name {
        INSPECT_PATCH => {
            let path = str_arg(args, "path")?;
            let v = file_ops::inspect(profile, catalog, path)
                .map_err(|e| McpError::Execution(e.to_string()))?;
            Ok(v.to_string())
        }
        "list_models" => {
            let block = str_arg(args, "block")?;
            Ok(file_ops::list_models_json(catalog, block).to_string())
        }
        "get_profile" => Ok(file_ops::profile_json(profile).to_string()),
        other if is_mcp_tool(other) => {
            let command =
                Command::parse(other, args).map_err(|e| McpError::BadArgument(e.to_string()))?;
            file_ops::execute_file_command(profile, catalog, &command)
                .map_err(|e| McpError::Execution(e.to_string()))
        }
        other => Err(McpError::UnknownTool(other.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (DeviceProfile, EffectCatalog) {
        mg101_pack_nux_mg101::load().unwrap()
    }

    fn tmpdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mg101_mcp_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn tool_list_has_file_tools_and_inspect_no_library_only() {
        let names: Vec<String> = tool_list().into_iter().map(|t| t.name).collect();
        for expected in [
            "set_parameter",
            "set_bpm",
            "list_models",
            "get_profile",
            "inspect_patch",
        ] {
            assert!(names.iter().any(|n| n == expected), "brak {expected}");
        }
        for absent in ["list_patches", "get_patch", "delete_patch", "import_patch"] {
            assert!(
                !names.iter().any(|n| n == absent),
                "nie powinno być {absent}"
            );
        }
    }

    #[test]
    fn set_parameter_required_input_output_in_schema() {
        let sp = tool_list()
            .into_iter()
            .find(|t| t.name == "set_parameter")
            .unwrap();
        let req = sp.input_schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "input"));
        assert!(req.iter().any(|v| v == "output"));
    }

    #[test]
    fn dispatch_get_profile_and_list_models_are_reads() {
        let (p, c) = fixture();
        let prof = dispatch(&p, &c, "get_profile", &Map::new()).unwrap();
        assert!(prof.contains("recordSize"));

        let mut a = Map::new();
        a.insert("block".into(), Value::String(p.blocks[0].id.clone()));
        let models = dispatch(&p, &c, "list_models", &a).unwrap();
        assert!(models.starts_with('['));
    }

    #[test]
    fn dispatch_set_bpm_then_inspect_roundtrips_through_files() {
        let (p, c) = fixture();
        let dir = tmpdir();
        let input = dir.join("mcp_in.mg101patch");
        let output = dir.join("mcp_out.mg101patch");
        let _ = std::fs::remove_file(&output);
        std::fs::write(&input, vec![0u8; p.record_size]).unwrap();

        let mut a = Map::new();
        a.insert(
            "input".into(),
            Value::String(input.to_string_lossy().into()),
        );
        a.insert(
            "output".into(),
            Value::String(output.to_string_lossy().into()),
        );
        a.insert("bpm".into(), Value::Number(96.into()));
        let msg = dispatch(&p, &c, "set_bpm", &a).unwrap();
        assert!(msg.contains("BPM"));

        let mut ia = Map::new();
        ia.insert(
            "path".into(),
            Value::String(output.to_string_lossy().into()),
        );
        let inspected = dispatch(&p, &c, INSPECT_PATCH, &ia).unwrap();
        assert!(inspected.contains("\"bpm\":96"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispatch_unknown_tool_errors() {
        let (p, c) = fixture();
        assert!(matches!(
            dispatch(&p, &c, "no_such_tool", &Map::new()),
            Err(McpError::UnknownTool(_))
        ));
    }

    #[test]
    fn dispatch_missing_path_is_bad_argument() {
        let (p, c) = fixture();
        assert!(matches!(
            dispatch(&p, &c, INSPECT_PATCH, &Map::new()),
            Err(McpError::BadArgument(_))
        ));
    }
}

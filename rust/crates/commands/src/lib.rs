//! Magistrala komend — jeden rejestr operacji domenowych.
//!
//! Port `DomainCommand` v1. Ten sam rejestr obsługuje UI (Slint), agenta
//! (tool-calling), MCP i przyszły web (ADR-0002, HLD §2). Każda komenda ma
//! klasę [`Kind`] sterującą autoryzacją (read/write/filesystem/destructive)
//! i wpisem WAL. Parser toleruje aliasy argumentów LLM (jak v1).

use serde_json::{Map, Value};

pub mod tool_defs;
pub use tool_defs::{ToolDefinition, Variant};

/// Identyfikator patcha (UUID lub `factory-NN`).
pub type PatchId = String;

/// Cel operacji: wpis biblioteki (id + oczekiwana rewizja) lub para plików.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetRef {
    Library {
        patch_id: PatchId,
        expected_revision: i64,
    },
    File {
        input: String,
        output: String,
    },
}

/// Klasa komendy — steruje autoryzacją i anotacjami MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
    Filesystem,
    Destructive,
}

/// Komenda domenowa (port `DomainCommand`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    // Read
    ListPatches,
    GetPatch {
        patch_id: PatchId,
    },
    GetSelection,
    ListModels {
        block: String,
    },
    GetProfile,
    GetDiff {
        patch_id: PatchId,
    },
    // Write
    SetParameter {
        target: TargetRef,
        block: String,
        parameter: String,
        value: i64,
    },
    SetModel {
        target: TargetRef,
        block: String,
        model: i64,
        values: Vec<i64>,
        bypassed: bool,
    },
    SetBypass {
        target: TargetRef,
        block: String,
        bypassed: bool,
    },
    SetName {
        target: TargetRef,
        name: String,
    },
    SetBpm {
        target: TargetRef,
        bpm: i64,
    },
    SetNamedField {
        target: TargetRef,
        field: String,
        value: i64,
    },
    ClearIr {
        target: TargetRef,
    },
    DuplicatePatch {
        target: TargetRef,
    },
    RevertLastAgentAction,
    // UI
    SelectPatch {
        patch_id: PatchId,
    },
    // Filesystem
    SetIr {
        target: TargetRef,
        wav_path: String,
        name: String,
    },
    ImportPatch {
        path: String,
    },
    ExportPatch {
        target: TargetRef,
        destination_path: String,
    },
    ListFiles {
        path: String,
    },
    // Destructive
    DeletePatch {
        target: TargetRef,
    },
    RevertSession,
}

impl Command {
    /// Nazwa narzędzia (snake_case) dla tej komendy.
    pub fn tool_name(&self) -> &'static str {
        match self {
            Command::ListPatches => "list_patches",
            Command::GetPatch { .. } => "get_patch",
            Command::GetSelection => "get_selection",
            Command::ListModels { .. } => "list_models",
            Command::GetProfile => "get_profile",
            Command::GetDiff { .. } => "get_diff",
            Command::SetParameter { .. } => "set_parameter",
            Command::SetModel { .. } => "set_model",
            Command::SetBypass { .. } => "set_bypass",
            Command::SetName { .. } => "set_name",
            Command::SetBpm { .. } => "set_bpm",
            Command::SetNamedField { .. } => "set_named_field",
            Command::ClearIr { .. } => "clear_ir",
            Command::DuplicatePatch { .. } => "duplicate_patch",
            Command::RevertLastAgentAction => "revert_last_agent_action",
            Command::SelectPatch { .. } => "select_patch",
            Command::SetIr { .. } => "set_ir",
            Command::ImportPatch { .. } => "import_patch",
            Command::ExportPatch { .. } => "export_patch",
            Command::ListFiles { .. } => "list_files",
            Command::DeletePatch { .. } => "delete_patch",
            Command::RevertSession => "revert_session",
        }
    }

    /// Klasa komendy (autoryzacja).
    pub fn kind(&self) -> Kind {
        match self {
            Command::ListPatches
            | Command::GetPatch { .. }
            | Command::GetSelection
            | Command::ListModels { .. }
            | Command::GetProfile
            | Command::GetDiff { .. }
            | Command::SelectPatch { .. } => Kind::Read,
            Command::SetParameter { .. }
            | Command::SetModel { .. }
            | Command::SetBypass { .. }
            | Command::SetName { .. }
            | Command::SetBpm { .. }
            | Command::SetNamedField { .. }
            | Command::ClearIr { .. }
            | Command::DuplicatePatch { .. }
            | Command::RevertLastAgentAction => Kind::Write,
            Command::SetIr { .. }
            | Command::ImportPatch { .. }
            | Command::ExportPatch { .. }
            | Command::ListFiles { .. } => Kind::Filesystem,
            Command::DeletePatch { .. } | Command::RevertSession => Kind::Destructive,
        }
    }
}

/// Błąd parsowania komendy (port `ToolParseError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    UnknownTool(String),
    MissingArgument(String),
    InvalidArgumentType { name: String, expected: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownTool(n) => write!(f, "Unknown tool: {n}"),
            ParseError::MissingArgument(n) => write!(f, "Missing required argument: {n}"),
            ParseError::InvalidArgumentType { name, expected } => {
                write!(f, "Invalid type for argument {name}, expected {expected}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

type Args = Map<String, Value>;

fn string_arg(args: &Args, key: &str) -> Result<String, ParseError> {
    match args.get(key) {
        None => Err(ParseError::MissingArgument(key.into())),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(_) => Err(ParseError::InvalidArgumentType {
            name: key.into(),
            expected: "string".into(),
        }),
    }
}

fn int_arg(args: &Args, key: &str) -> Result<i64, ParseError> {
    match args.get(key) {
        None => Err(ParseError::MissingArgument(key.into())),
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).ok_or(
            ParseError::InvalidArgumentType {
                name: key.into(),
                expected: "integer".into(),
            },
        ),
        Some(_) => Err(ParseError::InvalidArgumentType {
            name: key.into(),
            expected: "integer".into(),
        }),
    }
}

fn bool_arg(args: &Args, key: &str) -> Result<bool, ParseError> {
    match args.get(key) {
        None => Err(ParseError::MissingArgument(key.into())),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ParseError::InvalidArgumentType {
            name: key.into(),
            expected: "boolean".into(),
        }),
    }
}

fn int_array_arg(args: &Args, key: &str) -> Result<Vec<i64>, ParseError> {
    match args.get(key) {
        None => Err(ParseError::MissingArgument(key.into())),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| match v {
                // Jak v1 `Int(num)`: floaty (np. LLM emituje 50.0) są obcinane.
                Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).ok_or(
                    ParseError::InvalidArgumentType {
                        name: key.into(),
                        expected: "array of integers".into(),
                    },
                ),
                _ => Err(ParseError::InvalidArgumentType {
                    name: key.into(),
                    expected: "array of integers".into(),
                }),
            })
            .collect(),
        Some(_) => Err(ParseError::InvalidArgumentType {
            name: key.into(),
            expected: "array".into(),
        }),
    }
}

/// Pierwszy obecny alias klucza (tolerancja nazw argumentów LLM).
fn alias_string(args: &Args, keys: &[&str]) -> Result<String, ParseError> {
    let key = keys.iter().find(|k| args.contains_key(**k)).copied();
    string_arg(args, key.unwrap_or(keys[0]))
}

fn target_ref(args: &Args) -> Result<TargetRef, ParseError> {
    if args.contains_key("input") || args.contains_key("output") {
        Ok(TargetRef::File {
            input: string_arg(args, "input")?,
            output: string_arg(args, "output")?,
        })
    } else {
        Ok(TargetRef::Library {
            patch_id: string_arg(args, "patchID")?,
            expected_revision: int_arg(args, "expectedRevision")?,
        })
    }
}

impl Command {
    /// Parsuje komendę z nazwy narzędzia i argumentów (port `DomainCommand.parse`).
    pub fn parse(name: &str, args: &Args) -> Result<Command, ParseError> {
        Ok(match name {
            "list_patches" => Command::ListPatches,
            "get_patch" => Command::GetPatch {
                patch_id: string_arg(args, "patchID")?,
            },
            "get_selection" => Command::GetSelection,
            "list_models" => Command::ListModels {
                block: string_arg(args, "block")?,
            },
            "get_profile" => Command::GetProfile,
            "get_diff" => Command::GetDiff {
                patch_id: string_arg(args, "patchID")?,
            },
            "set_parameter" => Command::SetParameter {
                target: target_ref(args)?,
                block: string_arg(args, "block")?,
                parameter: string_arg(args, "parameter")?,
                value: int_arg(args, "value")?,
            },
            "set_model" => Command::SetModel {
                target: target_ref(args)?,
                block: string_arg(args, "block")?,
                model: int_arg(args, "model")?,
                values: int_array_arg(args, "values")?,
                bypassed: bool_arg(args, "bypassed")?,
            },
            "set_bypass" => Command::SetBypass {
                // Alias LLM: bypassed | bool_value.
                bypassed: bool_arg(args, "bypassed").or_else(|_| bool_arg(args, "bool_value"))?,
                target: target_ref(args)?,
                block: string_arg(args, "block")?,
            },
            "set_name" => Command::SetName {
                target: target_ref(args)?,
                name: string_arg(args, "name")?,
            },
            "set_bpm" => Command::SetBpm {
                target: target_ref(args)?,
                bpm: int_arg(args, "bpm")?,
            },
            "set_named_field" => Command::SetNamedField {
                // Alias LLM: field | parameter.
                field: alias_string(args, &["field", "parameter"])?,
                target: target_ref(args)?,
                value: int_arg(args, "value")?,
            },
            "clear_ir" => Command::ClearIr {
                target: target_ref(args)?,
            },
            "duplicate_patch" => Command::DuplicatePatch {
                target: target_ref(args)?,
            },
            "revert_last_agent_action" => Command::RevertLastAgentAction,
            "select_patch" => Command::SelectPatch {
                patch_id: string_arg(args, "patchID")?,
            },
            "set_ir" => Command::SetIr {
                // Alias LLM: wav | wavPath.
                wav_path: alias_string(args, &["wav", "wavPath"])?,
                target: target_ref(args)?,
                name: string_arg(args, "name")?,
            },
            "import_patch" => Command::ImportPatch {
                path: string_arg(args, "path")?,
            },
            "export_patch" => Command::ExportPatch {
                target: target_ref(args)?,
                destination_path: string_arg(args, "destinationPath")?,
            },
            "list_files" => Command::ListFiles {
                path: string_arg(args, "path")?,
            },
            "delete_patch" => Command::DeletePatch {
                target: target_ref(args)?,
            },
            "revert_session" => Command::RevertSession,
            other => return Err(ParseError::UnknownTool(other.into())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Args {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn parses_library_set_parameter() {
        let a = args(json!({
            "patchID": "p1", "expectedRevision": 3,
            "block": "amp", "parameter": "gain", "value": 50
        }));
        let cmd = Command::parse("set_parameter", &a).unwrap();
        assert_eq!(
            cmd,
            Command::SetParameter {
                target: TargetRef::Library {
                    patch_id: "p1".into(),
                    expected_revision: 3
                },
                block: "amp".into(),
                parameter: "gain".into(),
                value: 50
            }
        );
        assert_eq!(cmd.kind(), Kind::Write);
        assert_eq!(cmd.tool_name(), "set_parameter");
    }

    #[test]
    fn file_target_detected_by_input_output() {
        let a = args(json!({"input": "in.mg101patch", "output": "out.mg101patch", "bpm": 120}));
        let cmd = Command::parse("set_bpm", &a).unwrap();
        assert!(matches!(
            cmd,
            Command::SetBpm {
                target: TargetRef::File { .. },
                bpm: 120
            }
        ));
    }

    #[test]
    fn set_bypass_accepts_bool_value_alias() {
        let a = args(
            json!({"patchID": "p", "expectedRevision": 1, "block": "amp", "bool_value": true}),
        );
        let cmd = Command::parse("set_bypass", &a).unwrap();
        assert!(matches!(cmd, Command::SetBypass { bypassed: true, .. }));
    }

    #[test]
    fn set_named_field_accepts_parameter_alias() {
        let a =
            args(json!({"patchID": "p", "expectedRevision": 1, "parameter": "send", "value": 10}));
        let cmd = Command::parse("set_named_field", &a).unwrap();
        assert!(matches!(cmd, Command::SetNamedField { field, value: 10, .. } if field == "send"));
    }

    #[test]
    fn set_ir_accepts_wav_alias() {
        let a =
            args(json!({"patchID": "p", "expectedRevision": 1, "wav": "/x.wav", "name": "cab"}));
        let cmd = Command::parse("set_ir", &a).unwrap();
        assert_eq!(cmd.kind(), Kind::Filesystem);
        assert!(matches!(cmd, Command::SetIr { wav_path, .. } if wav_path == "/x.wav"));
    }

    #[test]
    fn kinds_are_classified() {
        assert_eq!(Command::ListPatches.kind(), Kind::Read);
        assert_eq!(
            Command::DeletePatch {
                target: TargetRef::Library {
                    patch_id: "p".into(),
                    expected_revision: 1
                }
            }
            .kind(),
            Kind::Destructive
        );
        assert_eq!(Command::RevertSession.kind(), Kind::Destructive);
    }

    #[test]
    fn unknown_tool_and_missing_arg_error() {
        assert!(matches!(
            Command::parse("nope", &args(json!({}))),
            Err(ParseError::UnknownTool(_))
        ));
        assert!(matches!(
            Command::parse("get_patch", &args(json!({}))),
            Err(ParseError::MissingArgument(_))
        ));
    }
}

//! Definicje narzędzi agenta — port `ToolDefinition` v1.
//!
//! Jeden generator schematów dla wariantu bibliotecznego (id+rewizja) i plikowego
//! (input/output). Klasa [`Kind`] steruje anotacjami. Formaty Anthropic i OpenAI
//! wyprowadzane z tego samego schematu (ADR-0001/0002: narzędzia z jednego źródła).

use crate::Kind;
use serde_json::{json, Value};

/// Wariant narzędzi: biblioteka (GUI/agent) lub pliki (MCP stdio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Library,
    File,
}

/// Definicja narzędzia: nazwa, opis, schemat wejścia (JSON Schema), klasa.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub kind: Kind,
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn str_prop(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}
fn int_prop(desc: &str) -> Value {
    json!({"type": "integer", "description": desc})
}
fn bool_prop(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

impl ToolDefinition {
    fn new(name: &str, description: &str, input_schema: Value, kind: Kind) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            kind,
        }
    }

    /// Format Anthropic (`name`/`description`/`input_schema`).
    pub fn anthropic_format(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        })
    }

    /// Format OpenAI (function calling).
    pub fn openai_format(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.input_schema,
            }
        })
    }

    /// Pełny zestaw narzędzi dla wariantu — port `allDefinitions(variant:)`.
    pub fn all(variant: Variant) -> Vec<ToolDefinition> {
        // Cel (target) zależny od wariantu.
        let (target_props, target_req): (Value, Vec<&'static str>) = match variant {
            Variant::Library => (
                json!({
                    "patchID": str_prop("ID patcha w bibliotece."),
                    "expectedRevision": int_prop("Oczekiwana rewizja patcha (unikanie konfliktów współbieżności)."),
                }),
                vec!["patchID", "expectedRevision"],
            ),
            Variant::File => (
                json!({
                    "input": str_prop("Ścieżka bezwzględna do pliku źródłowego .mg101patch."),
                    "output": str_prop("Ścieżka bezwzględna do docelowego pliku .mg101patch."),
                }),
                vec!["input", "output"],
            ),
        };
        // Pomocnik: scala pola celu z dodatkowymi.
        let with_target = |extra: Value| -> Value {
            let mut o = target_props.as_object().unwrap().clone();
            if let Value::Object(e) = extra {
                o.extend(e);
            }
            Value::Object(o)
        };
        let req = |extra: &[&'static str]| -> Vec<&'static str> {
            let mut r = target_req.clone();
            r.extend_from_slice(extra);
            r
        };

        let lib = variant == Variant::Library;
        let mut t = Vec::new();

        // READ
        t.push(ToolDefinition::new(
            "list_patches",
            "Wypisuje listę patchy w bibliotece (slot, ID, nazwa, origin, rewizja). Tylko tryb biblioteczny.",
            schema(json!({}), &[]),
            Kind::Read,
        ));
        t.push(ToolDefinition::new(
            "get_patch",
            "Pobiera szczegółowe dane patcha, w tym aktywne modele i parametry.",
            schema(json!({"patchID": str_prop("ID patcha")}), &["patchID"]),
            Kind::Read,
        ));
        t.push(ToolDefinition::new(
            "get_selection",
            "Zwraca aktualnie zaznaczony patch i blok w GUI. Tylko tryb biblioteczny.",
            schema(json!({}), &[]),
            Kind::Read,
        ));
        t.push(ToolDefinition::new(
            "list_models",
            "Wypisuje dostępne modele i parametry dla danego bloku (np. amp, efx).",
            schema(json!({"block": str_prop("ID bloku")}), &["block"]),
            Kind::Read,
        ));
        t.push(ToolDefinition::new(
            "get_profile",
            "Zwraca pełny profil aktywnego urządzenia (bloki, nazwane pola, ograniczenia).",
            schema(json!({}), &[]),
            Kind::Read,
        ));
        t.push(ToolDefinition::new(
            "get_diff",
            "Zwraca różnice bajtowe patcha względem oryginału. Tylko tryb biblioteczny.",
            schema(json!({"patchID": str_prop("ID patcha")}), &["patchID"]),
            Kind::Read,
        ));

        // WRITE
        t.push(ToolDefinition::new(
            "set_parameter",
            "Modyfikuje wskazany parametr w aktywnym modelu wybranego bloku.",
            schema(
                with_target(json!({
                    "block": str_prop("ID bloku (np. amp)."),
                    "parameter": str_prop("Nazwa parametru do zmiany."),
                    "value": int_prop("Nowa wartość parametru."),
                })),
                &req(&["block", "parameter", "value"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "set_model",
            "Zmienia aktywny model bloku i ustawia jego parametry.",
            schema(
                with_target(json!({
                    "block": str_prop("ID bloku (np. amp)."),
                    "model": int_prop("ID modelu dla tego bloku."),
                    "values": {"type": "array", "items": {"type": "integer"}, "description": "Wartości parametrów w kolejności katalogu efektów."},
                    "bypassed": bool_prop("Czy model ma być od razu pominięty (bypass)."),
                })),
                &req(&["block", "model", "values", "bypassed"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "set_bypass",
            "Włącza lub wyłącza (bypass) określony blok efektów.",
            schema(
                with_target(json!({
                    "block": str_prop("ID bloku."),
                    "bypassed": bool_prop("True jeśli blok ma być wyłączony (bypass)."),
                })),
                &req(&["block", "bypassed"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "set_name",
            "Zmienia nazwę wyświetlaną patcha.",
            schema(
                with_target(
                    json!({"name": str_prop("Nowa nazwa patcha (długość zależna od profilu).")}),
                ),
                &req(&["name"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "set_bpm",
            "Zmienia tempo BPM patcha.",
            schema(
                with_target(json!({"bpm": int_prop("Tempo BPM.")})),
                &req(&["bpm"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "set_named_field",
            "Modyfikuje globalne pole nazwane (np. send, return).",
            schema(
                with_target(json!({
                    "field": str_prop("Nazwa pola globalnego (np. send, return)."),
                    "value": int_prop("Wartość pola."),
                })),
                &req(&["field", "value"]),
            ),
            Kind::Write,
        ));
        t.push(ToolDefinition::new(
            "clear_ir",
            "Usuwa osadzony plik IR z wybranego patcha.",
            schema(with_target(json!({})), &req(&[])),
            Kind::Write,
        ));

        if lib {
            t.push(ToolDefinition::new(
                "duplicate_patch",
                "Tworzy kopię patcha w bibliotece i zwraca jej ID. Tylko tryb biblioteczny.",
                schema(with_target(json!({})), &req(&[])),
                Kind::Write,
            ));
            t.push(ToolDefinition::new(
                "revert_last_agent_action",
                "Cofa ostatnią operację mutującą agenta w bieżącej sesji. Tylko tryb biblioteczny.",
                schema(json!({}), &[]),
                Kind::Write,
            ));
            t.push(ToolDefinition::new(
                "select_patch",
                "Przełącza widok i zaznaczenie w GUI na wskazany patch.",
                schema(
                    json!({"patchID": str_prop("ID patcha do zaznaczenia")}),
                    &["patchID"],
                ),
                Kind::Read,
            ));
        }

        // FILESYSTEM
        let ir_key = if lib { "wavPath" } else { "wav" };
        // Wariant biblioteczny wskazuje agentowi sandbox ścieżek (jak v1) — to
        // element interfejsu behawioralnego LLM, nie tylko opis.
        let wav_desc = if lib {
            "Ścieżka bezwzględna do pliku WAV z zatwierdzonego katalogu."
        } else {
            "Ścieżka bezwzględna do pliku WAV."
        };
        t.push(ToolDefinition::new(
            "set_ir",
            "Osadza plik IR z formatu WAV we wskazanym patchu.",
            schema(
                with_target(json!({
                    ir_key: str_prop(wav_desc),
                    "name": str_prop("Nazwa IR (maks. 32 bajty)."),
                })),
                &req(&[ir_key, "name"]),
            ),
            Kind::Filesystem,
        ));
        if lib {
            t.push(ToolDefinition::new(
                "import_patch",
                "Importuje pojedynczy plik lub zestaw 36 patchy z zatwierdzonej ścieżki do biblioteki.",
                schema(json!({"path": str_prop("Ścieżka bezwzględna do pliku")}), &["path"]),
                Kind::Filesystem,
            ));
            t.push(ToolDefinition::new(
                "export_patch",
                "Eksportuje wybrany patch do nowego pliku w zatwierdzonym katalogu.",
                schema(
                    with_target(json!({"destinationPath": str_prop("Ścieżka katalogu lub pliku do eksportu.")})),
                    &req(&["destinationPath"]),
                ),
                Kind::Filesystem,
            ));
            t.push(ToolDefinition::new(
                "list_files",
                "Wypisuje listę plików w zatwierdzonym katalogu.",
                schema(
                    json!({"path": str_prop("Ścieżka bezwzględna do katalogu.")}),
                    &["path"],
                ),
                Kind::Filesystem,
            ));
            // DESTRUCTIVE
            t.push(ToolDefinition::new(
                "delete_patch",
                "Przenosi patch do kosza sesji (miękkie usunięcie).",
                schema(with_target(json!({})), &req(&[])),
                Kind::Destructive,
            ));
            t.push(ToolDefinition::new(
                "revert_session",
                "Cofa wszystkie zmiany wprowadzone w bieżącej sesji rozmowy.",
                schema(json!({}), &[]),
                Kind::Destructive,
            ));

            // LIVE DEVICE — DRUM (sterowanie na żywo, wymaga połączenia z urządzeniem)
            t.push(ToolDefinition::new(
                "drum_list_patterns",
                "Zwraca katalog wzorców perkusji DRUM: grupy (ROCK, CTRY, …) i wzorce w każdej grupie. Użyj, by poznać dostępne nazwy przed drum_set_pattern.",
                schema(json!({}), &[]),
                Kind::Read,
            ));
            t.push(ToolDefinition::new(
                "drum_transport",
                "Uruchamia lub zatrzymuje odtwarzanie perkusji na żywo na urządzeniu.",
                schema(
                    json!({"playing": bool_prop("true=Play, false=Stop.")}),
                    &["playing"],
                ),
                Kind::Write,
            ));
            t.push(ToolDefinition::new(
                "drum_set_volume",
                "Ustawia głośność perkusji (0–100) na żywo na urządzeniu.",
                schema(
                    json!({"value": int_prop("Głośność 0–100.")}),
                    &["value"],
                ),
                Kind::Write,
            ));
            t.push(ToolDefinition::new(
                "drum_set_pattern",
                "Wybiera wzorzec perkusji na żywo. Podaj nazwę grupy (np. ROCK, CTRY) oraz wzorzec jako nazwę (np. \"Walk Line\") lub numer 1-based w grupie.",
                schema(
                    json!({
                        "group": str_prop("Nazwa grupy: ROCK, CTRY, BLUES, METAL, FUNK, MET, BALD, POP, REGGAE, ELEC."),
                        "pattern": str_prop("Nazwa wzorca lub numer 1-based w obrębie grupy."),
                    }),
                    &["group", "pattern"],
                ),
                Kind::Write,
            ));
            t.push(ToolDefinition::new(
                "drum_set_tempo",
                "Ustawia tempo perkusji w BPM (40–240) na żywo na urządzeniu.",
                schema(json!({"bpm": int_prop("Tempo w BPM (40–240).")}), &["bpm"]),
                Kind::Write,
            ));
        }

        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_variant_has_full_toolset() {
        let tools = ToolDefinition::all(Variant::Library);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // Wszystkie narzędzia biblioteczne obecne.
        for expected in [
            "list_patches",
            "get_patch",
            "set_parameter",
            "set_model",
            "set_bypass",
            "duplicate_patch",
            "select_patch",
            "set_ir",
            "import_patch",
            "delete_patch",
            "revert_session",
        ] {
            assert!(names.contains(&expected), "brak {expected}");
        }
    }

    #[test]
    fn file_variant_excludes_library_only_tools() {
        let tools = ToolDefinition::all(Variant::File);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"import_patch"));
        assert!(!names.contains(&"delete_patch"));
        assert!(!names.contains(&"select_patch"));
        // set_parameter obecne, ale z celem plikowym.
        let sp = tools.iter().find(|t| t.name == "set_parameter").unwrap();
        let req = sp.input_schema["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "input"));
        assert!(req.iter().any(|v| v == "output"));
    }

    #[test]
    fn set_ir_key_differs_by_variant() {
        let lib = ToolDefinition::all(Variant::Library);
        let lib_ir = lib.iter().find(|t| t.name == "set_ir").unwrap();
        assert!(lib_ir.input_schema["properties"].get("wavPath").is_some());

        let file = ToolDefinition::all(Variant::File);
        let file_ir = file.iter().find(|t| t.name == "set_ir").unwrap();
        assert!(file_ir.input_schema["properties"].get("wav").is_some());
    }

    #[test]
    fn every_tool_parses_back_to_a_command() {
        // Spójność: każda wygenerowana nazwa narzędzia jest znana parserowi.
        for tool in ToolDefinition::all(Variant::Library) {
            // Minimalne argumenty nie są wymagane — sprawdzamy tylko, że nazwa
            // nie jest "UnknownTool" (parser rozpoznaje ją, choćby z błędem arg).
            let err = crate::Command::parse(&tool.name, &serde_json::Map::new());
            assert!(
                !matches!(err, Err(crate::ParseError::UnknownTool(_))),
                "parser nie zna narzędzia {}",
                tool.name
            );
        }
    }

    #[test]
    fn anthropic_and_openai_formats_wrap_schema() {
        let tool = &ToolDefinition::all(Variant::Library)[1]; // get_patch
        let a = tool.anthropic_format();
        assert_eq!(a["name"], "get_patch");
        assert!(a["input_schema"]["properties"].get("patchID").is_some());
        let o = tool.openai_format();
        assert_eq!(o["type"], "function");
        assert_eq!(o["function"]["name"], "get_patch");
    }
}

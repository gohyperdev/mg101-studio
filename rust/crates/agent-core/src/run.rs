//! Pętla tool-calling (port `AgentLoop.run`). Sterowana traitami: [`LlmClient`]
//! (transport modelu — mock w testach, HTTP w części 2), [`ToolExecutor`]
//! (wykonanie komendy) i [`Authorizer`] (potwierdzenia filesystem/destructive).
//! Autoryzacja i przepływ 1:1 z v1; sam rdzeń bez we/wy.

use crate::types::{ChatMessage, LlmResponse, Role, ToolResult, Usage};
use mg101_commands::{Command, Kind};
use serde_json::Value;

/// Maksymalna liczba iteracji pętli (port `maxIterations`).
pub const MAX_ITERATIONS: usize = 20;

/// Transport modelu — zwraca znormalizowaną odpowiedź dla podanej historii.
pub trait LlmClient {
    fn complete(&self, history: &[ChatMessage]) -> Result<LlmResponse, AgentError>;
}

/// Wykonawca komendy domenowej — zwraca JSON wyniku lub komunikat błędu.
pub trait ToolExecutor {
    fn execute(&mut self, command: &Command) -> Result<Value, String>;
}

/// Decyzja o autoryzacji operacji wrażliwej (filesystem/destructive).
pub trait Authorizer {
    /// Czy użytkownik zatwierdza wykonanie tej komendy.
    fn authorize(&mut self, command: &Command) -> bool;
}

/// Błąd pętli agenta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// Błąd transportu/dostawcy (HTTP, sieć, API).
    Provider(String),
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::Provider(m) => write!(f, "błąd dostawcy: {m}"),
        }
    }
}

/// Wynik przebiegu pętli.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub history: Vec<ChatMessage>,
    pub usage: Usage,
    pub iterations: usize,
}

/// Ścieżki systemu plików niesione przez komendę (do kontroli sandboxu).
fn command_paths(cmd: &Command) -> Vec<String> {
    match cmd {
        Command::ImportPatch { path } => vec![path.clone()],
        Command::ListFiles { path } => vec![path.clone()],
        Command::ExportPatch {
            destination_path, ..
        } => vec![destination_path.clone()],
        Command::SetIr { wav_path, .. } => vec![wav_path.clone()],
        _ => vec![],
    }
}

/// Katalog nadrzędny znormalizowanej ścieżki absolutnej (do zatwierdzenia
/// sandboxu). Zwraca `None` dla ścieżek względnych lub gdy rodzic to korzeń `/`
/// (nie zatwierdzamy całego systemu plików — domknięcie K1 z review E6).
fn parent_dir(path: &str) -> Option<String> {
    let norm = crate::config::normalize_lexical(path)?;
    match norm.rfind('/') {
        Some(0) | None => None, // rodzic to „/” — za szeroki, nie zatwierdzamy
        Some(i) => Some(norm[..i].to_string()),
    }
}

/// Uruchamia pętlę agenta. `history` musi już zawierać wiadomość użytkownika.
/// Iteruje: zapytanie modelu → jeśli brak wywołań narzędzi, kończy; inaczej
/// wykonuje narzędzia (z autoryzacją wrażliwych) i dokłada wyniki, aż model
/// przestanie wołać narzędzia lub do [`MAX_ITERATIONS`].
pub fn run<C: LlmClient, E: ToolExecutor, A: Authorizer>(
    client: &C,
    executor: &mut E,
    authorizer: &mut A,
    mut history: Vec<ChatMessage>,
    approved_roots: &mut Vec<String>,
) -> Result<RunOutcome, AgentError> {
    let mut usage = Usage::default();
    let mut iteration = 0;

    while iteration < MAX_ITERATIONS {
        iteration += 1;
        let response = client.complete(&history)?;
        usage.add(response.usage);

        if response.tool_calls.is_empty() {
            if let Some(text) = response.text.filter(|t| !t.is_empty()) {
                history.push(ChatMessage::assistant(text));
            }
            break;
        }

        // Wiadomość asystenta z wywołaniami narzędzi.
        history.push(ChatMessage {
            role: Role::Assistant,
            content: response.text.unwrap_or_default(),
            tool_calls: response.tool_calls.clone(),
            tool_results: vec![],
        });

        let mut results = Vec::new();
        for call in &response.tool_calls {
            let result = execute_call(
                executor,
                authorizer,
                approved_roots,
                &call.id,
                &call.name,
                &call.arguments,
            );
            results.push(result);
        }

        history.push(ChatMessage {
            role: Role::User,
            content: String::new(),
            tool_calls: vec![],
            tool_results: results,
        });
    }

    Ok(RunOutcome {
        history,
        usage,
        iterations: iteration,
    })
}

/// Wykonuje pojedyncze wywołanie narzędzia z autoryzacją; zawsze zwraca
/// [`ToolResult`] (błędy trafiają do modelu jako `is_error`, jak w v1).
fn execute_call<E: ToolExecutor, A: Authorizer>(
    executor: &mut E,
    authorizer: &mut A,
    approved_roots: &mut Vec<String>,
    call_id: &str,
    name: &str,
    args: &serde_json::Map<String, Value>,
) -> ToolResult {
    let err_result = |msg: String| ToolResult {
        tool_use_id: call_id.to_string(),
        content: msg,
        is_error: true,
    };

    let cmd = match Command::parse(name, args) {
        Ok(c) => c,
        Err(e) => return err_result(e.to_string()),
    };

    // Autoryzacja wg klasy komendy.
    match cmd.kind() {
        Kind::Filesystem => {
            for p in command_paths(&cmd) {
                // Ścieżki muszą być absolutne i znormalizowane (blokada traversal).
                let Some(parent) = parent_dir(&p) else {
                    return err_result(format!(
                        "Odrzucono ścieżkę '{p}': wymagana ścieżka absolutna bez '..'."
                    ));
                };
                if !crate::config::is_path_approved(&p, approved_roots) {
                    if authorizer.authorize(&cmd) {
                        approved_roots.push(parent);
                    } else {
                        return err_result(format!("Odmowa dostępu do ścieżki '{p}'."));
                    }
                }
            }
        }
        Kind::Destructive => {
            if !authorizer.authorize(&cmd) {
                return err_result("Operacja anulowana przez użytkownika.".into());
            }
        }
        Kind::Read | Kind::Write => {}
    }

    match executor.execute(&cmd) {
        Ok(v) => ToolResult {
            tool_use_id: call_id.to_string(),
            // Klucze sortowane — deterministyczny wynik (jak `.sortedKeys` v1).
            content: sorted_json(&v),
            is_error: false,
        },
        Err(msg) => err_result(msg),
    }
}

/// Serializuje `Value` z posortowanymi kluczami (deterministycznie).
fn sorted_json(v: &Value) -> String {
    // serde_json z BTreeMap-em zachowuje porządek; przejdźmy przez wartość.
    fn canon(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: std::collections::BTreeMap<String, Value> =
                    m.iter().map(|(k, val)| (k.clone(), canon(val))).collect();
                serde_json::to_value(sorted).unwrap_or(Value::Null)
            }
            Value::Array(a) => Value::Array(a.iter().map(canon).collect()),
            other => other.clone(),
        }
    }
    canon(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LlmResponse, ToolCall};
    use serde_json::json;
    use std::cell::RefCell;

    /// Klient zwracający zaprogramowaną sekwencję odpowiedzi.
    struct ScriptedClient {
        responses: RefCell<std::collections::VecDeque<LlmResponse>>,
    }
    impl ScriptedClient {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: RefCell::new(responses.into()),
            }
        }
    }
    impl LlmClient for ScriptedClient {
        fn complete(&self, _history: &[ChatMessage]) -> Result<LlmResponse, AgentError> {
            self.responses
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| AgentError::Provider("brak zaprogramowanej odpowiedzi".into()))
        }
    }

    /// Wykonawca rejestrujący komendy; zwraca stały wynik.
    #[derive(Default)]
    struct RecordingExecutor {
        executed: Vec<String>,
    }
    impl ToolExecutor for RecordingExecutor {
        fn execute(&mut self, command: &Command) -> Result<Value, String> {
            self.executed.push(command.tool_name().to_string());
            Ok(json!({"ok": true}))
        }
    }

    /// Autoryzator ze stałą decyzją.
    struct FixedAuth(bool);
    impl Authorizer for FixedAuth {
        fn authorize(&mut self, _command: &Command) -> bool {
            self.0
        }
    }

    fn call(id: &str, name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: args.as_object().unwrap().clone(),
        }
    }

    fn text_response(t: &str) -> LlmResponse {
        LlmResponse {
            text: Some(t.into()),
            tool_calls: vec![],
            usage: Usage {
                input_tokens: 5,
                output_tokens: 3,
            },
        }
    }

    #[test]
    fn stops_when_no_tool_calls() {
        let client = ScriptedClient::new(vec![text_response("Gotowe")]);
        let mut exec = RecordingExecutor::default();
        let mut auth = FixedAuth(true);
        let out = run(
            &client,
            &mut exec,
            &mut auth,
            vec![ChatMessage::user("cześć")],
            &mut vec![],
        )
        .unwrap();
        assert_eq!(out.iterations, 1);
        assert_eq!(out.history.last().unwrap().content, "Gotowe");
        assert_eq!(out.usage.input_tokens, 5);
    }

    #[test]
    fn executes_tool_then_continues_to_final_text() {
        let with_tool = LlmResponse {
            text: Some("Ustawiam".into()),
            tool_calls: vec![call(
                "t1",
                "set_bpm",
                json!({"patchID": "p", "expectedRevision": 1, "bpm": 120}),
            )],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 4,
            },
        };
        let client = ScriptedClient::new(vec![with_tool, text_response("Zrobione")]);
        let mut exec = RecordingExecutor::default();
        let mut auth = FixedAuth(true);
        let out = run(
            &client,
            &mut exec,
            &mut auth,
            vec![ChatMessage::user("bpm 120")],
            &mut vec![],
        )
        .unwrap();
        assert_eq!(out.iterations, 2);
        assert_eq!(exec.executed, vec!["set_bpm"]);
        // Historia: user, assistant(+toolcall), user(toolresult), assistant(final).
        assert_eq!(out.history.len(), 4);
        let tr = &out.history[2].tool_results[0];
        assert!(!tr.is_error);
        assert_eq!(tr.content, "{\"ok\":true}");
        assert_eq!(out.usage.output_tokens, 7); // 4 + 3
    }

    #[test]
    fn destructive_denied_yields_error_result_not_execution() {
        let with_tool = LlmResponse {
            text: None,
            tool_calls: vec![call(
                "t1",
                "delete_patch",
                json!({"patchID": "p", "expectedRevision": 1}),
            )],
            usage: Usage::default(),
        };
        let client = ScriptedClient::new(vec![with_tool, text_response("koniec")]);
        let mut exec = RecordingExecutor::default();
        let mut auth = FixedAuth(false); // odmowa
        let out = run(
            &client,
            &mut exec,
            &mut auth,
            vec![ChatMessage::user("skasuj")],
            &mut vec![],
        )
        .unwrap();
        assert!(exec.executed.is_empty()); // nie wykonano
        let tr = &out.history[2].tool_results[0];
        assert!(tr.is_error);
    }

    #[test]
    fn filesystem_path_approved_after_consent_and_reused() {
        let import1 = LlmResponse {
            text: None,
            tool_calls: vec![call(
                "t1",
                "import_patch",
                json!({"path": "/data/a.mg101patch"}),
            )],
            usage: Usage::default(),
        };
        let import2 = LlmResponse {
            text: None,
            tool_calls: vec![call(
                "t2",
                "import_patch",
                json!({"path": "/data/b.mg101patch"}),
            )],
            usage: Usage::default(),
        };
        let client = ScriptedClient::new(vec![import1, import2, text_response("koniec")]);
        let mut exec = RecordingExecutor::default();

        struct CountingAuth {
            calls: usize,
        }
        impl Authorizer for CountingAuth {
            fn authorize(&mut self, _c: &Command) -> bool {
                self.calls += 1;
                true
            }
        }
        let mut auth = CountingAuth { calls: 0 };
        let mut roots = vec![];
        let out = run(
            &client,
            &mut exec,
            &mut auth,
            vec![ChatMessage::user("importuj")],
            &mut roots,
        )
        .unwrap();
        // Druga ścieżka w tym samym katalogu /data nie pyta ponownie.
        assert_eq!(auth.calls, 1);
        assert_eq!(exec.executed, vec!["import_patch", "import_patch"]);
        assert!(roots.contains(&"/data".to_string()));
        assert_eq!(out.iterations, 3);
    }

    #[test]
    fn loop_bounded_by_max_iterations() {
        // Model bez końca woła narzędzie — pętla musi się zatrzymać.
        let looping: Vec<LlmResponse> = (0..MAX_ITERATIONS + 5)
            .map(|_| LlmResponse {
                text: None,
                tool_calls: vec![call("t", "get_profile", json!({}))],
                usage: Usage::default(),
            })
            .collect();
        let client = ScriptedClient::new(looping);
        let mut exec = RecordingExecutor::default();
        let mut auth = FixedAuth(true);
        let out = run(
            &client,
            &mut exec,
            &mut auth,
            vec![ChatMessage::user("loop")],
            &mut vec![],
        )
        .unwrap();
        assert_eq!(out.iterations, MAX_ITERATIONS);
    }
}

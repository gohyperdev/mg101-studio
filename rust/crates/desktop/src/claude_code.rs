//! Backend agenta oparty o **lokalnie zainstalowany Claude Code** (`claude -p`).
//!
//! Po co: Anthropic nie udostępnia OAuth, którym aplikacja trzeciej strony mogłaby
//! zużywać subskrypcję Claude Pro/Max — więc zamiast udawać cudzego klienta,
//! wołamy program, który użytkownik ma już zalogowany. Płaci jego subskrypcja,
//! klucz API jest niepotrzebny.
//!
//! Narzędzia: nie wystawiamy ich przez nasz kanał — Claude Code prowadzi własną
//! pętlę narzędziową i sięga do **żywego Studio** przez serwer MCP
//! (`mg101-mcp --bridge` → [`crate::bridge`]). Dlatego backend jest bezużyteczny
//! przy wyłączonym mostku i mówimy o tym wprost, zamiast po cichu odbierać agentowi ręce.
//!
//! Uprawnienia: przekazujemy wyłącznie `--allowed-tools mcp__mg101`. Nasze narzędzia
//! są dozwolone, wszystko inne (Bash, Read, zapis plików) zostaje **odrzucone** —
//! świadomie nie używamy `--dangerously-skip-permissions`.

use std::io::{BufRead, BufReader};
use std::process::{Command as OsCommand, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread::JoinHandle;

use mg101_agent_core::Usage;
use serde_json::{json, Value};

/// Nazwa serwera MCP w konfiguracji przekazywanej Claude Code. Z niej wynikają
/// nazwy narzędzi widziane przez model (`mcp__mg101__set_bpm`) i prefiks w
/// `--allowed-tools`, więc zmiana tutaj musi iść w parze z [`ALLOWED_TOOLS`].
const SERVER_NAME: &str = "mg101";
const ALLOWED_TOOLS: &str = "mcp__mg101";

/// Kontekst domeny dokładany do promptu systemowego Claude Code (jego własny
/// prompt zostaje — dopisujemy tylko rolę i kontrakt narzędzi).
const APPEND_SYSTEM: &str = "Pracujesz w aplikacji MG101 Studio (edytor patchy \
    gitarowego procesora NUX MG-101). Patche zmieniasz WYŁĄCZNIE narzędziami \
    MCP `mg101` — działają na żywej Bibliotece uruchomionej aplikacji, więc \
    użytkownik widzi skutki natychmiast. Przy operacjach mutujących podawaj \
    `patchID` oraz `expectedRevision` (rewizję odczytaną przed zmianą). \
    Nie używaj powłoki ani plików. Na koniec zwięźle podsumuj, co zmieniłeś.";

/// Zdarzenie z wątku Claude Code do UI.
#[derive(Debug, Clone, PartialEq)]
pub enum CcEvent {
    /// Zaczęła się sesja — id do wznowienia kolejnej tury (`--resume`).
    Session(String),
    /// Model sięgnął po narzędzie (do wskaźnika postępu).
    Tool(String),
    /// Koniec przebiegu: tekst odpowiedzi + zużycie.
    Done { text: String, usage: Usage },
    /// Błąd uruchomienia lub przebiegu.
    Error(String),
}

/// Uchwyt biegnącego przebiegu Claude Code (analogiczny do `ChatRunner`, ale bez
/// kanału narzędzi — te obsługuje MCP → mostek).
pub struct ClaudeCodeRunner {
    event_rx: Receiver<CcEvent>,
    _join: JoinHandle<()>,
}

/// Co uruchomić i jak. `session` = kontynuacja rozmowy (`--resume`).
pub struct CcOptions {
    pub claude_bin: String,
    pub model: String,
    pub session: Option<String>,
    /// Katalog roboczy procesu — celowo katalog danych aplikacji, nie projekt
    /// użytkownika: Claude Code nie ma po co widzieć cudzych plików.
    pub work_dir: std::path::PathBuf,
}

impl ClaudeCodeRunner {
    /// Startuje `claude -p` na osobnym wątku i strumieniuje zdarzenia do UI.
    pub fn start(prompt: String, opts: CcOptions) -> Self {
        let (tx, event_rx) = channel();

        let join = std::thread::spawn(move || {
            let cfg = match write_mcp_config(&opts.work_dir) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(CcEvent::Error(e));
                    return;
                }
            };

            let mut cmd = OsCommand::new(&opts.claude_bin);
            cmd.arg("-p")
                .arg(&prompt)
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose") // stream-json w trybie -p tego wymaga
                .arg("--mcp-config")
                .arg(&cfg)
                .arg("--allowed-tools")
                .arg(ALLOWED_TOOLS)
                .arg("--append-system-prompt")
                .arg(APPEND_SYSTEM);
            if !opts.model.trim().is_empty() {
                cmd.arg("--model").arg(&opts.model);
            }
            if let Some(sid) = &opts.session {
                cmd.arg("--resume").arg(sid);
            }
            cmd.current_dir(&opts.work_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let dbg = std::env::var_os("MG101_AGENT_DEBUG").is_some();
            if dbg {
                eprintln!("[cc] {} -p … --mcp-config {}", opts.claude_bin, cfg.display());
            }

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(CcEvent::Error(spawn_error(&opts.claude_bin, &e)));
                    return;
                }
            };

            // stderr zbieramy równolegle — inaczej pełny bufor potrafi zakleszczyć proces.
            let err_handle = child.stderr.take().map(|e| {
                std::thread::spawn(move || {
                    BufReader::new(e)
                        .lines()
                        .map_while(Result::ok)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            });

            let mut finished = false;
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if dbg {
                        eprintln!("[cc] < {}", &line[..line.len().min(140)]);
                    }
                    if let Some(ev) = parse_line(&line) {
                        finished |= matches!(ev, CcEvent::Done { .. } | CcEvent::Error(_));
                        if tx.send(ev).is_err() {
                            let _ = child.kill(); // UI zniknęło — nie zostawiaj sieroty
                            return;
                        }
                    }
                }
            }

            let status = child.wait();
            let stderr = err_handle.and_then(|h| h.join().ok()).unwrap_or_default();
            let _ = std::fs::remove_file(&cfg);

            // Cisza na stdout mimo końca procesu = coś padło zanim model odpowiedział.
            if !finished {
                let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
                let _ = tx.send(CcEvent::Error(format!(
                    "Claude Code zakończył się bez odpowiedzi (kod {code}). {}",
                    tail(&stderr)
                )));
            }
        });

        Self {
            event_rx,
            _join: join,
        }
    }

    /// Nieblokująco pobiera zdarzenie (jeśli jest).
    pub fn try_event(&self) -> Option<CcEvent> {
        self.event_rx.try_recv().ok()
    }
}

/// Czytelny komunikat, gdy binarki nie ma — to najczęstszy błąd konfiguracji.
fn spawn_error(bin: &str, e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!(
            "nie znaleziono Claude Code ({bin}). Zainstaluj go i zaloguj (`claude`), \
             albo podaj pełną ścieżkę do binarki w Ustawieniach."
        )
    } else {
        format!("nie udało się uruchomić {bin}: {e}")
    }
}

/// Ostatnie linie stderr — pełny log Claude Code potrafi być bardzo długi.
fn tail(stderr: &str) -> String {
    let t: Vec<&str> = stderr.lines().rev().take(3).collect();
    t.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// Ścieżka do naszej binarki MCP. Kolejność: zmienna środowiskowa (obejście dla
/// nietypowych instalacji) → obok binarki aplikacji (bundle i `cargo run`) → `PATH`.
pub fn mcp_bin_path() -> String {
    if let Some(v) = std::env::var_os("MG101_MCP_BIN") {
        return v.to_string_lossy().into_owned();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("mg101-mcp");
            if sibling.is_file() {
                return sibling.to_string_lossy().into_owned();
            }
        }
    }
    "mg101-mcp".into()
}

/// Konfiguracja MCP dla Claude Code: jeden serwer, tryb mostka (żywa aplikacja).
fn mcp_config_json(bin: &str) -> Value {
    json!({
        "mcpServers": {
            SERVER_NAME: { "command": bin, "args": ["--bridge"] }
        }
    })
}

/// Zapisuje konfigurację MCP do pliku (Claude Code przyjmuje ścieżkę).
fn write_mcp_config(dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("katalog danych: {e}"))?;
    let path = dir.join("claude-mcp.json");
    let text = mcp_config_json(&mcp_bin_path()).to_string();
    std::fs::write(&path, text).map_err(|e| format!("zapis konfiguracji MCP: {e}"))?;
    Ok(path)
}

/// Zamienia jedną linię `--output-format stream-json` na zdarzenie UI.
/// Linie, które nas nie interesują (np. `user` z wynikami narzędzi), dają `None`.
fn parse_line(line: &str) -> Option<CcEvent> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    match v.get("type")?.as_str()? {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            let sid = v.get("session_id")?.as_str()?;
            Some(CcEvent::Session(sid.to_string()))
        }
        // Nazwa narzędzia tylko do wskaźnika postępu — treść wywołania nas nie obchodzi.
        "assistant" => {
            let blocks = v.get("message")?.get("content")?.as_array()?;
            let name = blocks
                .iter()
                .find(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
                .and_then(|b| b.get("name"))
                .and_then(Value::as_str)?;
            Some(CcEvent::Tool(short_tool(name)))
        }
        "result" => {
            let usage = Usage {
                input_tokens: v
                    .pointer("/usage/input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: v
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            };
            let text = v.get("result").and_then(Value::as_str).unwrap_or_default();
            let failed = v.get("is_error").and_then(Value::as_bool) == Some(true)
                || v.get("subtype").and_then(Value::as_str) != Some("success");
            if failed {
                let why = if text.is_empty() {
                    v.get("subtype")
                        .and_then(Value::as_str)
                        .unwrap_or("nieznany błąd")
                } else {
                    text
                };
                Some(CcEvent::Error(format!("Claude Code: {why}")))
            } else {
                Some(CcEvent::Done {
                    text: text.to_string(),
                    usage,
                })
            }
        }
        _ => None,
    }
}

/// `mcp__mg101__set_bpm` → `set_bpm` (w pasku postępu prefiks to szum).
fn short_tool(name: &str) -> String {
    name.rsplit("__").next().unwrap_or(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_line_yields_session_id() {
        let l = r#"{"type":"system","subtype":"init","session_id":"abc-123","tools":[]}"#;
        assert_eq!(parse_line(l), Some(CcEvent::Session("abc-123".into())));
    }

    #[test]
    fn tool_use_is_reported_without_prefix() {
        let l = r#"{"type":"assistant","message":{"content":[
            {"type":"text","text":"Zaraz zmienię"},
            {"type":"tool_use","id":"t1","name":"mcp__mg101__set_bpm","input":{"bpm":137}}]}}"#;
        assert_eq!(parse_line(l), Some(CcEvent::Tool("set_bpm".into())));
    }

    #[test]
    fn assistant_text_without_tools_is_not_progress() {
        let l = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}"#;
        assert_eq!(parse_line(l), None);
    }

    #[test]
    fn success_result_carries_text_and_usage() {
        let l = r#"{"type":"result","subtype":"success","is_error":false,
            "result":"Ustawiłem BPM na 137.","session_id":"s","usage":{"input_tokens":12,"output_tokens":3}}"#;
        assert_eq!(
            parse_line(l),
            Some(CcEvent::Done {
                text: "Ustawiłem BPM na 137.".into(),
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 3
                },
            })
        );
    }

    #[test]
    fn error_result_becomes_error_not_empty_answer() {
        // Regresja: `is_error` bez tekstu nie może przejść jako pusta odpowiedź agenta.
        let l = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#;
        match parse_line(l) {
            Some(CcEvent::Error(e)) => assert!(e.contains("error_during_execution"), "{e}"),
            other => panic!("oczekiwano błędu, jest {other:?}"),
        }
    }

    #[test]
    fn garbage_lines_are_skipped_not_fatal() {
        assert_eq!(parse_line("nie-json"), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line(r#"{"type":"user","message":{}}"#), None);
    }

    #[test]
    fn mcp_config_points_at_the_bridge() {
        // Bez `--bridge` narzędzia edytowałyby pliki, a nie żywą aplikację —
        // agent raportowałby sukces, a w UI nic by się nie zmieniało.
        let c = mcp_config_json("/opt/mg101-mcp");
        assert_eq!(c["mcpServers"]["mg101"]["command"], "/opt/mg101-mcp");
        assert_eq!(c["mcpServers"]["mg101"]["args"][0], "--bridge");
    }

    #[test]
    fn allowed_tools_prefix_matches_server_name() {
        assert_eq!(ALLOWED_TOOLS, format!("mcp__{SERVER_NAME}"));
    }

    #[test]
    fn missing_binary_gives_actionable_message() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "x");
        let msg = spawn_error("claude", &e);
        assert!(msg.contains("Ustawieniach"), "{msg}");
    }
}

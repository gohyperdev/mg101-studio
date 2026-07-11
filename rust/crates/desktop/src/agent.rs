//! Wątek agenta dla UI (natywny) — port pętli agenta v1 do modelu Slint.
//!
//! Pętla [`mg101_agent_core::run`] jest blokująca (HTTP), więc biegnie na
//! **osobnym wątku**. Wykonanie narzędzi MUSI trafić do współdzielonego składu na
//! wątku UI, dlatego egzekutor agenta jest **kanałowy**: wysyła [`Command`] do UI
//! i blokuje na odpowiedzi. Wątek UI obsługuje te żądania nieblokująco (Timer),
//! wykonując je na `ViewModel`/`Studio`. Dzięki temu `Studio` pozostaje
//! jednowątkowe (bez `Mutex`), a UI się nie zawiesza.
//!
//! Natywny (wątki + HTTP); nie wchodzi do wasm.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;

use mg101_agent_core::{
    run, AgentConfig, Authorizer, ChatMessage, HttpLlmClient, ToolExecutor, Usage,
};
use mg101_commands::{Command, Variant};
use serde_json::Value;

/// Domyślny prompt systemowy agenta (rola + kontrakt narzędzi).
pub const SYSTEM_PROMPT: &str = "Jesteś asystentem edycji patchy gitarowego \
    procesora NUX MG-101 w aplikacji MG101 Studio. Edytujesz patche w bibliotece \
    przez dostępne narzędzia (ustawianie modeli, parametrów, bypassu, nazwy, BPM, \
    IR; duplikowanie, usuwanie). Zawsze podawaj `patchID` i `expectedRevision` \
    (rewizję patcha, którą właśnie odczytałeś) przy operacjach mutujących. \
    Po zakończeniu zwięźle podsumuj, co zmieniłeś.";

/// Zdarzenie z wątku agenta do UI.
pub enum AgentEvent {
    /// Przebieg zakończony — pełna historia i zużycie tokenów.
    Done {
        history: Vec<ChatMessage>,
        usage: Usage,
    },
    /// Błąd dostawcy/transportu.
    Error(String),
}

/// Egzekutor kanałowy (na wątku agenta): każde narzędzie wysyła komendę do UI i
/// blokuje na odpowiedzi.
struct ChannelExecutor {
    cmd_tx: Sender<Command>,
    res_rx: Receiver<Result<Value, String>>,
}

impl ToolExecutor for ChannelExecutor {
    fn execute(&mut self, command: &Command) -> Result<Value, String> {
        self.cmd_tx
            .send(command.clone())
            .map_err(|_| "UI zamknięte".to_string())?;
        self.res_rx.recv().map_err(|_| "UI zamknięte".to_string())?
    }
}

/// Autoryzator: zatwierdza wszystko. Sandbox ścieżek egzekwuje `run()`
/// (znormalizowane `approved_roots`), a destrukcyjne operacje na bibliotece są
/// miękkie/odwracalne (revert). Per-operacyjne pytanie UI → backlog.
struct AutoAuthorizer;

impl Authorizer for AutoAuthorizer {
    fn authorize(&mut self, _command: &Command) -> bool {
        true
    }
}

/// Uchwyt biegnącego przebiegu agenta na wątku UI.
pub struct ChatRunner {
    cmd_rx: Receiver<Command>,
    res_tx: Sender<Result<Value, String>>,
    event_rx: Receiver<AgentEvent>,
    _join: JoinHandle<()>,
}

impl ChatRunner {
    /// Startuje przebieg: buduje klienta HTTP z konfiguracji i uruchamia pętlę na
    /// osobnym wątku. `history` musi już zawierać wiadomość użytkownika.
    pub fn start(config: AgentConfig, system: String, history: Vec<ChatMessage>) -> Self {
        let (cmd_tx, cmd_rx) = channel(); // agent → UI (żądania narzędzi)
        let (res_tx, res_rx) = channel(); // UI → agent (wyniki narzędzi)
        let (event_tx, event_rx) = channel(); // agent → UI (koniec/błąd)

        let join = std::thread::spawn(move || {
            let client = HttpLlmClient::new(config, system, Variant::Library);
            let mut executor = ChannelExecutor { cmd_tx, res_rx };
            let mut auth = AutoAuthorizer;
            let mut roots: Vec<String> = Vec::new();
            let event = match run(&client, &mut executor, &mut auth, history, &mut roots) {
                Ok(outcome) => AgentEvent::Done {
                    history: outcome.history,
                    usage: outcome.usage,
                },
                Err(e) => AgentEvent::Error(e.to_string()),
            };
            let _ = event_tx.send(event);
        });

        Self {
            cmd_rx,
            res_tx,
            event_rx,
            _join: join,
        }
    }

    /// Nieblokująco pobiera oczekujące żądanie narzędzia (jeśli jest).
    pub fn try_tool_request(&self) -> Option<Command> {
        self.cmd_rx.try_recv().ok()
    }

    /// Odsyła wynik narzędzia do wątku agenta.
    pub fn send_tool_result(&self, result: Result<Value, String>) {
        let _ = self.res_tx.send(result);
    }

    /// Nieblokująco pobiera zdarzenie końcowe/błąd (jeśli jest).
    pub fn try_event(&self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }
}

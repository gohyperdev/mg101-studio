//! Mostek MCP → **żywa aplikacja**: wystawia działające `Studio` (Biblioteka,
//! zaznaczenie, urządzenie) klientom zewnętrznym, takim jak Claude Code.
//!
//! Dlaczego mostek, a nie serwer MCP wprost w aplikacji: `Studio` jest
//! **jednowątkowe** (Rc/RefCell na wątku UI — ADR-0002, bez Mutexów). Mostek
//! przyjmuje żądania na wątkach sieciowych i przekazuje je **kanałem** na wątek UI,
//! który wykonuje je na `Studio` i odsyła wynik. Ten sam wzorzec co `ChannelExecutor`
//! agenta, tylko w drugą stronę.
//!
//! Protokół (JSON po liniach, TCP):
//!   → `{"token":"…","tool":"set_bpm","args":{…}}`
//!   ← `{"ok":true,"result":{…}}`  albo  `{"ok":false,"error":"…"}`
//!
//! Bezpieczeństwo: nasłuch **wyłącznie na 127.0.0.1**, każde żądanie musi podać
//! token wygenerowany przy starcie. Token trafia do pliku `~/.mg101_bridge_token`
//! (prawa 0600) — tylko procesy tego użytkownika go odczytają. Token NIGDY nie
//! trafia do logów.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

/// Pierwszy próbowany port (parytet z v1); przy zajętym skanujemy w górę.
const PREFERRED_PORT: u16 = 10101;
const PORT_SCAN: u16 = 100;

/// Liczniki mostka — dzielone z wątkami sieciowymi, czytane przez UI.
/// Bez nich zakładka MCP mówiłaby tylko „mostek włączony”, a użytkownik i tak
/// nie wiedziałby, czy zewnętrzny agent jest **podłączony** i czy coś robi.
#[derive(Default)]
struct Stats {
    /// Ilu klientów trzyma otwarte połączenie *teraz*.
    clients: AtomicUsize,
    /// Ile żądań obsłużyliśmy od startu.
    requests: AtomicU64,
    /// Ile odrzucono na złym tokenie — to sygnał bezpieczeństwa, nie ciekawostka.
    rejected: AtomicU64,
    /// Ostatnie narzędzie + moment jego wywołania.
    last: Mutex<Option<(String, std::time::Instant)>>,
}

/// Migawka stanu mostka dla UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeStatus {
    pub port: u16,
    pub clients: usize,
    pub requests: u64,
    pub rejected: u64,
    /// Ostatnio wywołane narzędzie i ile sekund temu.
    pub last_tool: Option<(String, u64)>,
}

/// Żądanie z mostka do wątku UI: narzędzie + argumenty + kanał na odpowiedź.
pub struct BridgeRequest {
    pub tool: String,
    pub args: serde_json::Map<String, Value>,
    reply: Sender<Result<Value, String>>,
}

impl BridgeRequest {
    /// Odsyła wynik wykonania do czekającego klienta.
    pub fn respond(self, result: Result<Value, String>) {
        let _ = self.reply.send(result);
    }
}

/// Uchwyt działającego mostka. Trzymany przez aplikację; upuszczenie NIE zamyka
/// wątku nasłuchu (proces i tak kończy się z aplikacją), ale kanał się rozłącza.
pub struct Bridge {
    requests: Receiver<BridgeRequest>,
    pub port: u16,
    stats: Arc<Stats>,
}

impl Bridge {
    /// Startuje nasłuch na localhost i zapisuje port+token do plików w `$HOME`.
    /// Zwraca `None`, gdy nie da się otworzyć żadnego portu (mostek jest opcjonalny —
    /// aplikacja musi działać dalej bez niego).
    pub fn start() -> Option<Self> {
        let token = random_token();
        let (listener, port) = bind_local()?;
        write_secret(".mg101_bridge_port", &port.to_string());
        write_secret(".mg101_bridge_token", &token);

        let (tx, requests) = channel::<BridgeRequest>();
        let stats = Arc::new(Stats::default());
        let listen_stats = stats.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let tx = tx.clone();
                let token = token.clone();
                let stats = listen_stats.clone();
                std::thread::spawn(move || serve_client(stream, tx, token, stats));
            }
        });
        Some(Self {
            requests,
            port,
            stats,
        })
    }

    /// Odbiera oczekujące żądania (nieblokująco) — wołane z pompy wątku UI.
    pub fn poll(&self) -> Vec<BridgeRequest> {
        self.requests.try_iter().collect()
    }

    /// Migawka dla zakładki MCP: czy ktoś jest podłączony i co ostatnio robił.
    pub fn status(&self) -> BridgeStatus {
        let last = self
            .stats
            .last
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .map(|(tool, at)| (tool, at.elapsed().as_secs()));
        BridgeStatus {
            port: self.port,
            clients: self.stats.clients.load(Ordering::Relaxed),
            requests: self.stats.requests.load(Ordering::Relaxed),
            rejected: self.stats.rejected.load(Ordering::Relaxed),
            last_tool: last,
        }
    }
}

/// Obsługa jednego klienta: linia = jedno żądanie, odpowiedź = jedna linia.
fn serve_client(stream: TcpStream, tx: Sender<BridgeRequest>, token: String, stats: Arc<Stats>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    // Licznik żywych klientów: `Connected` w UI ma znaczyć „gniazdo otwarte teraz”,
    // więc dekrementujemy na KAŻDYM wyjściu z funkcji (także przy zerwaniu połączenia).
    stats.clients.fetch_add(1, Ordering::Relaxed);
    let _guard = ClientGuard(stats.clone());

    let reader = BufReader::new(stream);
    let mut out = write_half;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = handle_line(&line, &tx, &token);
        record(&stats, &line, &response);
        let mut payload = response.to_string();
        payload.push('\n');
        if out.write_all(payload.as_bytes()).is_err() {
            break;
        }
        let _ = out.flush();
    }
}

/// Opis stanu mostka dla zakładki MCP: (nagłówek, szczegóły).
/// `None` = mostek wyłączony w Ustawieniach albo nie wstał.
pub fn describe(status: Option<&BridgeStatus>, lang: crate::Lang) -> (String, String) {
    let t = |k| crate::i18n::tr(lang, k);
    let Some(s) = status else {
        return (t("mcp.off").to_string(), t("mcp.off_hint").to_string());
    };

    let head = if s.clients > 0 {
        format!("{} ({})", t("mcp.connected"), s.clients)
    } else {
        t("mcp.idle").to_string()
    };

    let mut detail = format!(
        "127.0.0.1:{} · {} {}",
        s.port,
        t("mcp.requests"),
        s.requests
    );
    if let Some((tool, ago)) = &s.last_tool {
        detail.push_str(&format!(" · {} {tool} ({ago} {})", t("mcp.last"), t("mcp.ago")));
    }
    // Odrzucenia pokazujemy TYLKO, gdy wystąpiły — to sygnał, że ktoś puka bez tokenu.
    if s.rejected > 0 {
        detail.push_str(&format!(" · {} {}", t("mcp.rejected"), s.rejected));
    }
    (head, detail)
}

/// Zmniejsza licznik klientów przy wyjściu (także awaryjnym).
struct ClientGuard(Arc<Stats>);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.0.clients.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Dopisuje żądanie do liczników. Odrzucenia na tokenie liczymy osobno —
/// nie są „ruchem agenta”, tylko sygnałem, że ktoś puka bez klucza.
fn record(stats: &Stats, line: &str, response: &Value) {
    if response.get("error").and_then(Value::as_str) == Some("zły token") {
        stats.rejected.fetch_add(1, Ordering::Relaxed);
        return;
    }
    stats.requests.fetch_add(1, Ordering::Relaxed);
    let tool = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("tool").and_then(Value::as_str).map(str::to_owned));
    if let (Some(tool), Ok(mut slot)) = (tool, stats.last.lock()) {
        *slot = Some((tool, std::time::Instant::now()));
    }
}

fn handle_line(line: &str, tx: &Sender<BridgeRequest>, token: &str) -> Value {
    let Ok(req) = serde_json::from_str::<Value>(line) else {
        return json!({"ok": false, "error": "niepoprawny JSON"});
    };
    // Token sprawdzamy PRZED czymkolwiek innym — bez niego nie zdradzamy nawet,
    // czy narzędzie istnieje.
    let given = req.get("token").and_then(Value::as_str).unwrap_or_default();
    if !constant_time_eq(given, token) {
        return json!({"ok": false, "error": "zły token"});
    }
    let Some(tool) = req.get("tool").and_then(Value::as_str) else {
        return json!({"ok": false, "error": "brak pola 'tool'"});
    };
    let args = req
        .get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let (reply_tx, reply_rx) = channel();
    let sent = tx.send(BridgeRequest {
        tool: tool.to_string(),
        args,
        reply: reply_tx,
    });
    if sent.is_err() {
        return json!({"ok": false, "error": "aplikacja zamknięta"});
    }
    // Czekamy na wątek UI. Limit chroni klienta przed zawisem, gdyby UI utknęło.
    match reply_rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(result)) => json!({"ok": true, "result": result}),
        Ok(Err(e)) => json!({"ok": false, "error": e}),
        Err(_) => json!({"ok": false, "error": "brak odpowiedzi aplikacji (timeout)"}),
    }
}

/// Porównanie w stałym czasie — token nie może wyciekać przez czas odpowiedzi.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Nasłuch tylko na pętli zwrotnej (nigdy 0.0.0.0 — mostek mutuje Bibliotekę).
fn bind_local() -> Option<(TcpListener, u16)> {
    for port in PREFERRED_PORT..PREFERRED_PORT.saturating_add(PORT_SCAN) {
        if let Ok(l) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            return Some((l, port));
        }
    }
    None
}

/// Losowy token 32-hex ze źródła systemowego.
fn random_token() -> String {
    // Bez dodatkowej zależności: /dev/urandom (macOS/Linux). Awaryjnie — czas+pid,
    // co jest słabsze, ale mostek i tak stoi tylko na localhost za tokenem.
    let mut buf = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf))
        .is_err()
    {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> (i % 4 * 8)) ^ (pid >> (i % 4 * 8))) as u8;
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Zapisuje sekret do `$HOME/<name>` z prawami 0600 (tylko właściciel).
fn write_secret(name: &str, value: &str) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = std::path::Path::new(&home).join(name);
    if std::fs::write(&path, value).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Odczytuje port i token mostka (dla klienta — np. `mg101-mcp --bridge`).
/// `None`, gdy aplikacja nie działa albo nie wystawiła mostka.
pub fn discover() -> Option<(u16, String)> {
    let home = std::env::var_os("HOME")?;
    let home = std::path::Path::new(&home);
    let port = std::fs::read_to_string(home.join(".mg101_bridge_port"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = std::fs::read_to_string(home.join(".mg101_bridge_token"))
        .ok()?
        .trim()
        .to_string();
    if token.is_empty() {
        return None;
    }
    Some((port, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_hex_chars() {
        let t = random_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        // Dwa kolejne tokeny nie mogą być takie same.
        assert_ne!(t, random_token());
    }

    #[test]
    fn constant_time_eq_matches_semantics_of_eq() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn bad_token_is_rejected_without_touching_the_channel() {
        let (tx, rx) = channel::<BridgeRequest>();
        let v = handle_line(r#"{"token":"zly","tool":"list_patches"}"#, &tx, "dobry");
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "zły token");
        // Kluczowe: żądanie z błędnym tokenem NIE dociera do Studio.
        assert!(rx.try_recv().is_err(), "żądanie nie może trafić do aplikacji");
    }

    #[test]
    fn stats_count_requests_and_remember_last_tool() {
        let stats = Stats::default();
        let ok = json!({"ok": true, "result": {}});
        record(&stats, r#"{"token":"t","tool":"list_patches"}"#, &ok);
        record(&stats, r#"{"token":"t","tool":"set_bpm"}"#, &ok);
        assert_eq!(stats.requests.load(Ordering::Relaxed), 2);
        assert_eq!(stats.rejected.load(Ordering::Relaxed), 0);
        let (tool, _) = stats.last.lock().unwrap().clone().expect("ostatnie narzędzie");
        assert_eq!(tool, "set_bpm", "status ma pokazywać NAJNOWSZE wywołanie");
    }

    #[test]
    fn bad_token_counts_as_rejected_not_as_traffic() {
        // Inaczej „Żądania: 5” sugerowałoby pracę agenta, gdy to ktoś dobija się bez klucza.
        let stats = Stats::default();
        let denied = json!({"ok": false, "error": "zły token"});
        record(&stats, r#"{"token":"zly","tool":"set_bpm"}"#, &denied);
        assert_eq!(stats.rejected.load(Ordering::Relaxed), 1);
        assert_eq!(stats.requests.load(Ordering::Relaxed), 0);
        assert!(stats.last.lock().unwrap().is_none(), "odrzucone nie jest ostatnim narzędziem");
    }

    #[test]
    fn describe_distinguishes_off_idle_and_connected() {
        use crate::Lang;
        let (head, detail) = describe(None, Lang::Pl);
        assert!(head.contains("wyłączony"), "{head}");
        assert!(!detail.is_empty(), "wyłączony mostek musi mówić, jak go włączyć");

        let idle = BridgeStatus {
            port: 10101,
            clients: 0,
            requests: 0,
            rejected: 0,
            last_tool: None,
        };
        let (head, detail) = describe(Some(&idle), Lang::Pl);
        assert!(head.contains("Mostek aktywny"), "{head}");
        assert!(detail.contains("127.0.0.1:10101"), "{detail}");

        let busy = BridgeStatus {
            port: 10101,
            clients: 2,
            requests: 9,
            rejected: 0,
            last_tool: Some(("set_bpm".into(), 3)),
        };
        let (head, detail) = describe(Some(&busy), Lang::Pl);
        assert!(head.contains("(2)"), "nagłówek ma pokazać liczbę klientów: {head}");
        assert!(detail.contains("set_bpm") && detail.contains("3"), "{detail}");
    }

    #[test]
    fn rejected_shown_only_when_nonzero() {
        use crate::Lang;
        let clean = BridgeStatus {
            port: 1,
            clients: 0,
            requests: 1,
            rejected: 0,
            last_tool: None,
        };
        let (_, d) = describe(Some(&clean), Lang::En);
        assert!(!d.contains("reject"), "brak odrzuceń → brak wzmianki: {d}");
        let attacked = BridgeStatus {
            rejected: 4,
            ..clean
        };
        let (_, d) = describe(Some(&attacked), Lang::En);
        assert!(d.contains('4'), "odrzucenia muszą być widoczne: {d}");
    }

    #[test]
    fn client_guard_decrements_on_disconnect() {
        let stats = Arc::new(Stats::default());
        stats.clients.fetch_add(1, Ordering::Relaxed);
        {
            let _g = ClientGuard(stats.clone());
            assert_eq!(stats.clients.load(Ordering::Relaxed), 1);
        }
        assert_eq!(stats.clients.load(Ordering::Relaxed), 0, "rozłączenie musi zdjąć klienta");
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let (tx, _rx) = channel::<BridgeRequest>();
        assert_eq!(handle_line("nie-json", &tx, "t")["ok"], false);
        assert_eq!(handle_line(r#"{"token":"t"}"#, &tx, "t")["error"], "brak pola 'tool'");
    }

    #[test]
    fn valid_request_reaches_ui_thread_and_returns_result() {
        let (tx, rx) = channel::<BridgeRequest>();
        // Udajemy wątek UI: odbierz żądanie i odpowiedz.
        let ui = std::thread::spawn(move || {
            let req = rx.recv().expect("żądanie");
            assert_eq!(req.tool, "set_bpm");
            assert_eq!(req.args["bpm"], 137);
            req.respond(Ok(json!({"success": true})));
        });
        let v = handle_line(
            r#"{"token":"t","tool":"set_bpm","args":{"bpm":137}}"#,
            &tx,
            "t",
        );
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["success"], true);
        ui.join().unwrap();
    }
}

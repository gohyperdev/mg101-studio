//! Klient mostka: łączy serwer MCP z **działającą aplikacją** zamiast z plikami.
//!
//! W trybie plikowym każde narzędzie czyta `input` i zapisuje `output` — zmiany są
//! niewidoczne dla uruchomionej aplikacji. W trybie mostkowym narzędzia są
//! **przekazywane do żywego `Studio`**, więc edycje widać w UI natychmiast, a agent
//! ma dostęp do Biblioteki, zaznaczenia i urządzenia (wariant `Library`).
//!
//! Protokół: JSON po liniach po TCP (patrz `mg101_desktop::bridge`).
//! Port i token aplikacja zapisuje do `~/.mg101_bridge_port` / `~/.mg101_bridge_token`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Mutex;

use serde_json::{json, Value};

/// Połączenie z mostkiem aplikacji. Jedno gniazdo, żądania szeregowane —
/// aplikacja i tak wykonuje je po kolei na wątku UI, więc zrównoleglanie nic nie da.
pub struct BridgeClient {
    stream: Mutex<Option<TcpStream>>,
    port: u16,
    token: String,
}

impl BridgeClient {
    /// Odczytuje port i token z plików aplikacji i łączy się z mostkiem.
    /// `Err` z czytelnym powodem, gdy aplikacja nie działa lub mostek jest wyłączony.
    pub fn connect() -> Result<Self, String> {
        let (port, token) = discover()
            .ok_or("nie znaleziono mostka: uruchom MG101 Studio i włącz mostek w Ustawieniach")?;
        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("mostek na porcie {port} nie odpowiada ({e}) — czy aplikacja działa?"))?;
        Ok(Self {
            stream: Mutex::new(Some(stream)),
            port,
            token,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wywołuje narzędzie na żywym `Studio` aplikacji.
    pub fn call(&self, tool: &str, args: &serde_json::Map<String, Value>) -> Result<Value, String> {
        let mut req = json!({"token": self.token, "tool": tool, "args": args});
        let mut line = req.take().to_string();
        line.push('\n');

        let mut guard = self.stream.lock().map_err(|_| "mostek zatruty".to_string())?;
        let stream = guard.as_mut().ok_or("mostek rozłączony")?;

        stream
            .write_all(line.as_bytes())
            .map_err(|e| format!("zapis do mostka: {e}"))?;
        stream
            .flush()
            .map_err(|e| format!("wysyłka do mostka: {e}"))?;

        // Czytamy DOKŁADNIE jedną linię odpowiedzi. BufReader tworzony na sklonowanym
        // uchwycie, żeby nie zjeść bajtów należących do kolejnego żądania.
        let peer = stream.try_clone().map_err(|e| format!("klon gniazda: {e}"))?;
        let mut reader = BufReader::new(peer);
        let mut resp = String::new();
        let n = reader
            .read_line(&mut resp)
            .map_err(|e| format!("odczyt z mostka: {e}"))?;
        if n == 0 {
            *guard = None;
            return Err("mostek zamknął połączenie (aplikacja zakończona?)".into());
        }

        let v: Value = serde_json::from_str(resp.trim())
            .map_err(|e| format!("niepoprawna odpowiedź mostka: {e}"))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("nieznany błąd mostka")
                .to_string())
        }
    }
}

/// Port + token mostka zapisane przez aplikację.
fn discover() -> Option<(u16, String)> {
    let home = std::env::var_os("HOME")?;
    let home = std::path::Path::new(&home);
    let port: u16 = std::fs::read_to_string(home.join(".mg101_bridge_port"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = std::fs::read_to_string(home.join(".mg101_bridge_token"))
        .ok()?
        .trim()
        .to_string();
    (!token.is_empty()).then_some((port, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    /// Serwer-atrapa mostka: odbiera jedno żądanie i odsyła ustaloną odpowiedź.
    fn fake_bridge(response: &'static str) -> (u16, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap();
            let got = String::from_utf8_lossy(&buf[..n]).to_string();
            s.write_all(response.as_bytes()).unwrap();
            s.flush().unwrap();
            got
        });
        (port, h)
    }

    fn client_for(port: u16) -> BridgeClient {
        BridgeClient {
            stream: Mutex::new(Some(TcpStream::connect(("127.0.0.1", port)).unwrap())),
            port,
            token: "tok".into(),
        }
    }

    #[test]
    fn sends_token_and_returns_result() {
        let (port, h) = fake_bridge("{\"ok\":true,\"result\":{\"revision\":7}}\n");
        let c = client_for(port);
        let mut args = serde_json::Map::new();
        args.insert("bpm".into(), json!(137));
        let out = c.call("set_bpm", &args).unwrap();
        assert_eq!(out["revision"], 7);

        let sent: Value = serde_json::from_str(h.join().unwrap().trim()).unwrap();
        assert_eq!(sent["token"], "tok", "token musi lecieć w każdym żądaniu");
        assert_eq!(sent["tool"], "set_bpm");
        assert_eq!(sent["args"]["bpm"], 137);
    }

    #[test]
    fn bridge_error_surfaces_as_err() {
        let (port, h) = fake_bridge("{\"ok\":false,\"error\":\"zły token\"}\n");
        let c = client_for(port);
        let e = c.call("list_patches", &serde_json::Map::new()).unwrap_err();
        assert_eq!(e, "zły token");
        h.join().unwrap();
    }

    #[test]
    fn closed_connection_is_reported_not_panic() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            drop(s); // natychmiastowe zamknięcie
        });
        let c = client_for(port);
        let e = c.call("list_patches", &serde_json::Map::new()).unwrap_err();
        // Zerwane gniazdo daje — zależnie od wyścigu z zamknięciem po drugiej
        // stronie — EOF („mostek zamknął połączenie”) albo ECONNRESET przy
        // zapisie/wysyłce/odczycie („… do mostka”). Każda ścieżka ma dać czytelny
        // błąd, nie panikę, więc sprawdzamy wspólny rdzeń słowa.
        assert!(
            e.contains("most"),
            "czytelny komunikat o mostku, dostaliśmy: {e}"
        );
    }
}

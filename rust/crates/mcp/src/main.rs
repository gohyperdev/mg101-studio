//! Binarka serwera MCP MG-101 po stdio.
//!
//! Dwa tryby (konfigurowalne przy uruchomieniu):
//!
//! - `--bridge` — most do **działającej aplikacji**: narzędzia operują na żywej
//!   Bibliotece, więc zmiany widać w UI natychmiast (wariant `Library`).
//! - `--file` — edycja **plików** `.mg101patch` (`input` → `output`). Aplikacja
//!   nie widzi tych zmian (wariant `File`).
//! - bez flagi (`auto`, domyślnie) — mostek, jeśli aplikacja działa; inaczej pliki.
//!
//! Logi diagnostyczne idą na stderr (stdout jest kanałem protokołu — nie zaśmiecać).

use rmcp::{transport::io::stdio, ServiceExt};

use mg101_mcp::bridge_client::BridgeClient;
use mg101_mcp::server::McpServer;

enum Mode {
    Auto,
    Bridge,
    File,
}

fn parse_args() -> Result<Mode, String> {
    let mut mode = Mode::Auto;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--bridge" => mode = Mode::Bridge,
            "--file" => mode = Mode::File,
            "--auto" => mode = Mode::Auto,
            "-h" | "--help" => {
                eprintln!(
                    "mg101-mcp — serwer MCP dla NUX MG-101\n\n\
                     UŻYCIE: mg101-mcp [--bridge | --file | --auto]\n\n\
                     --bridge  narzędzia działają na ŻYWEJ aplikacji (zmiany widać w UI)\n\
                     --file    edycja plików .mg101patch (aplikacja NIE widzi zmian)\n\
                     --auto    mostek, jeśli aplikacja działa; inaczej pliki [domyślne]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("nieznany argument: {other} (użyj --help)")),
        }
    }
    Ok(mode)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = parse_args()?;
    let (profile, catalog) = mg101_pack_nux_mg101::load()?;

    let server = match mode {
        // Jawny mostek: brak aplikacji to BŁĄD (użytkownik prosił o tryb live —
        // ciche zejście do plików wyglądałoby jak działające, a nic by nie zmieniało).
        Mode::Bridge => {
            let client = BridgeClient::connect()?;
            eprintln!("mg101-mcp: most do aplikacji (port {})", client.port());
            McpServer::bridged(profile, catalog, client)
        }
        Mode::File => {
            eprintln!("mg101-mcp: tryb plikowy (zmiany NIE trafiają do aplikacji)");
            McpServer::new(profile, catalog)
        }
        Mode::Auto => match BridgeClient::connect() {
            Ok(client) => {
                eprintln!("mg101-mcp: most do aplikacji (port {})", client.port());
                McpServer::bridged(profile, catalog, client)
            }
            Err(e) => {
                eprintln!("mg101-mcp: tryb plikowy ({e})");
                McpServer::new(profile, catalog)
            }
        },
    };

    let running = server.serve(stdio()).await?;
    let reason = running.waiting().await?;
    eprintln!("mg101-mcp: koniec ({reason:?})");
    Ok(())
}

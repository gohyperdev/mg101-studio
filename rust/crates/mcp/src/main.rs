//! Binarka serwera MCP MG-101 po stdio (port `Sources/MG101MCP/main.swift`).
//!
//! Ładuje profil/katalog z packa NUX MG-101 i obsługuje sesję MCP na stdin/stdout.
//! Logi diagnostyczne idą na stderr (stdout jest kanałem protokołu — nie zaśmiecać).

use rmcp::{transport::io::stdio, ServiceExt};

use mg101_mcp::server::McpServer;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (profile, catalog) = mg101_pack_nux_mg101::load()?;
    let server = McpServer::new(profile, catalog);

    eprintln!("mg101-mcp: start (stdio)");
    let running = server.serve(stdio()).await?;
    let reason = running.waiting().await?;
    eprintln!("mg101-mcp: koniec ({reason:?})");
    Ok(())
}

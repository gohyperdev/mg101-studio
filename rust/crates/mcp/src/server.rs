//! Powłoka rmcp: opakowuje czysty dyspozytor ([`crate::dispatch`]) w
//! [`ServerHandler`] po stdio. Cała logika narzędzi jest w [`crate`] (testowana
//! bez async/sieci) — tu tylko mapowanie typów rmcp i pętla transportu.

use std::sync::Arc;

use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, Content, Implementation, ListToolsResult,
        PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
        ToolAnnotations,
    },
    service::{RequestContext, RoleServer},
    ErrorData as RmcpError, ServerHandler,
};

use mg101_commands::Kind;
use mg101_core::{DeviceProfile, EffectCatalog};

use crate::bridge_client::BridgeClient;
use crate::{dispatch, tool_list_for, McpError, McpTool};
use mg101_commands::Variant;

/// Tryb pracy serwera.
#[derive(Clone)]
enum Mode {
    /// Edycja PLIKÓW `.mg101patch` (`input` → `output`). Aplikacja NIE widzi zmian.
    File,
    /// Most do DZIAŁAJĄCEJ aplikacji: narzędzia idą na żywe `Studio`, więc zmiany
    /// widać w UI natychmiast, a agent ma Bibliotekę, zaznaczenie i urządzenie.
    Bridge(Arc<BridgeClient>),
}

/// Serwer MCP: profil + katalog urządzenia (wstrzykiwane przez pack — bez wiedzy
/// o MG-101 w kodzie).
#[derive(Clone)]
pub struct McpServer {
    profile: Arc<DeviceProfile>,
    catalog: Arc<EffectCatalog>,
    mode: Mode,
}

impl McpServer {
    /// Tryb plikowy (domyślny).
    pub fn new(profile: DeviceProfile, catalog: EffectCatalog) -> Self {
        Self {
            profile: Arc::new(profile),
            catalog: Arc::new(catalog),
            mode: Mode::File,
        }
    }

    /// Tryb mostkowy — wymaga działającej aplikacji z włączonym mostkiem.
    pub fn bridged(profile: DeviceProfile, catalog: EffectCatalog, client: BridgeClient) -> Self {
        Self {
            profile: Arc::new(profile),
            catalog: Arc::new(catalog),
            mode: Mode::Bridge(Arc::new(client)),
        }
    }

    fn variant(&self) -> Variant {
        match self.mode {
            Mode::File => Variant::File,
            Mode::Bridge(_) => Variant::Library,
        }
    }
}

/// Anotacje MCP z klasy komendy (parytet v1: `readOnlyHint`=read,
/// `destructiveHint`=destructive, `openWorldHint`=filesystem).
fn annotations(kind: Kind) -> ToolAnnotations {
    ToolAnnotations {
        read_only_hint: Some(kind == Kind::Read),
        destructive_hint: Some(kind == Kind::Destructive),
        open_world_hint: Some(kind == Kind::Filesystem),
        // v1 ustawiał jawnie idempotentHint:false (main.swift) — parytet.
        idempotent_hint: Some(false),
        ..Default::default()
    }
}

fn to_tool(t: McpTool) -> Tool {
    Tool::new(t.name, t.description, Arc::new(t.input_schema)).annotate(annotations(t.kind))
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "mg101-mcp".into(),
                title: Some(match self.mode {
                    Mode::File => "NUX MG-101 Studio (pliki)".into(),
                    Mode::Bridge(_) => "NUX MG-101 Studio (żywa aplikacja)".to_string(),
                }),
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                website_url: None,
            },
            instructions: Some(match self.mode {
                Mode::File => {
                    "Edycja plików .mg101patch: każde narzędzie mutujące czyta `input` i \
                     zapisuje `output` (nie nadpisuje istniejącego). `inspect_patch` \
                     odczytuje plik, `list_models`/`get_profile` opisują urządzenie. \
                     UWAGA: zmiany NIE są widoczne w działającej aplikacji."
                        .to_string()
                }
                Mode::Bridge(_) => "Edycja ŻYWEJ Biblioteki działającej aplikacji MG101 Studio — \
                     zmiany widać w UI natychmiast. Operuj na `patchID` i `expectedRevision` \
                     (rewizję bierz z `list_patches`/`get_patch`). Dostępne też zaznaczenie w GUI \
                     (`get_selection`, `select_patch`) i sterowanie urządzeniem na żywo (drum_*)."
                    .to_string(),
            }),
        }
    }

    async fn list_tools(
        &self,
        _req: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, RmcpError> {
        Ok(ListToolsResult::with_all_items(
            tool_list_for(self.variant())
                .into_iter()
                .map(to_tool)
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, RmcpError> {
        let args = req.arguments.unwrap_or_default();
        // Tryb mostkowy: narzędzie wykonuje APLIKACJA na swoim żywym Studio.
        if let Mode::Bridge(client) = &self.mode {
            return Ok(match client.call(&req.name, &args) {
                Ok(v) => CallToolResult::success(vec![Content::text(
                    serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string()),
                )]),
                Err(e) => CallToolResult::error(vec![Content::text(e)]),
            });
        }
        match dispatch(&self.profile, &self.catalog, &req.name, &args) {
            Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
            // Nieznane narzędzie / zły argument to błąd protokołu (nie tool-error).
            Err(e @ (McpError::UnknownTool(_) | McpError::BadArgument(_))) => {
                Err(RmcpError::invalid_params(e.to_string(), None))
            }
            // Błąd wykonania (I/O, walidacja) → tool-error z treścią (widoczny dla LLM).
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }
}

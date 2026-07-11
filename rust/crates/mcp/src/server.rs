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

use crate::{dispatch, tool_list, McpError, McpTool};

/// Serwer MCP: profil + katalog urządzenia (wstrzykiwane przez pack — bez wiedzy
/// o MG-101 w kodzie).
#[derive(Clone)]
pub struct McpServer {
    profile: Arc<DeviceProfile>,
    catalog: Arc<EffectCatalog>,
}

impl McpServer {
    pub fn new(profile: DeviceProfile, catalog: EffectCatalog) -> Self {
        Self {
            profile: Arc::new(profile),
            catalog: Arc::new(catalog),
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
                title: Some("NUX MG-101 Studio (pliki)".into()),
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "Edycja plików .mg101patch: każde narzędzie mutujące czyta `input` i \
                 zapisuje `output` (nie nadpisuje istniejącego). `inspect_patch` \
                 odczytuje plik, `list_models`/`get_profile` opisują urządzenie."
                    .into(),
            ),
        }
    }

    async fn list_tools(
        &self,
        _req: Option<PaginatedRequestParam>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, RmcpError> {
        Ok(ListToolsResult::with_all_items(
            tool_list().into_iter().map(to_tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParam,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, RmcpError> {
        let args = req.arguments.unwrap_or_default();
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

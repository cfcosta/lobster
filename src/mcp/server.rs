//! MCP server built on `rmcp` — the official Rust MCP SDK.
//!
//! Exposes Lobster's memory tools via stdio transport.

use std::sync::Arc;

use grafeo::GrafeoDB;
use rmcp::{
    ServerHandler,
    ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool,
    tool_handler,
    tool_router,
};

use crate::{mcp::tools, store::db::LobsterDb};

/// Request parameters for tools that take a query string.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct QueryRequest {
    /// Natural-language query describing the current task or question.
    pub query: String,
}

/// Request parameters for the `memory_neighbors` tool.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct NeighborsRequest {
    /// ID of the node to get neighbors for.
    pub node_id: String,
}

/// The Lobster MCP server handler.
#[derive(Clone)]
pub struct LobsterServer {
    db: Arc<LobsterDb>,
    grafeo: Arc<GrafeoDB>,
    tool_router: ToolRouter<Self>,
}

impl LobsterServer {
    /// Create a new server with the given database and graph.
    #[must_use]
    pub fn new(db: Arc<LobsterDb>, grafeo: Arc<GrafeoDB>) -> Self {
        Self {
            db,
            grafeo,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl LobsterServer {
    #[tool(
        description = "Task-oriented context bundle: returns ranked decisions, summaries, tasks, and entities for the current situation."
    )]
    fn memory_context(
        &self,
        Parameters(req): Parameters<QueryRequest>,
    ) -> String {
        let bundle = tools::memory_context(&req.query, &self.db, &self.grafeo);
        serde_json::to_string(&bundle).unwrap_or_default()
    }

    #[tool(
        description = "List the newest ready artifacts (episodes, decisions, tasks)."
    )]
    fn memory_recent(&self) -> String {
        let result = tools::memory_recent(&self.db, None);
        serde_json::to_string(&result).unwrap_or_default()
    }

    #[tool(
        description = "Search memory for ranked hits with snippets and confidence scores."
    )]
    fn memory_search(
        &self,
        Parameters(req): Parameters<QueryRequest>,
    ) -> String {
        let result = tools::memory_search(&req.query, &self.db, &self.grafeo);
        serde_json::to_string(&result).unwrap_or_default()
    }

    #[tool(description = "Return decision timeline with rationale.")]
    fn memory_decisions(&self) -> String {
        let result = tools::memory_decisions(&self.db, None);
        serde_json::to_string(&result).unwrap_or_default()
    }

    #[tool(description = "Graph neighbor traversal from a given entity node.")]
    fn memory_neighbors(
        &self,
        Parameters(req): Parameters<NeighborsRequest>,
    ) -> String {
        let result = tools::memory_neighbors(&self.grafeo, &req.node_id);
        serde_json::to_string(&result).unwrap_or_default()
    }

    #[tool(
        description = "Repo identity profile: stable conventions and user preferences detected from the episode stream."
    )]
    fn memory_profile(&self) -> String {
        let result = tools::memory_profile(&self.db);
        serde_json::to_string(&result).unwrap_or_default()
    }

    #[tool(
        description = "Processing state diagnostics: episode counts, artifacts, pending/failed status, profile facts."
    )]
    fn memory_status(&self) -> String {
        let report = crate::app::status::scan(&self.db);
        let workflows = crate::store::crud::list_tool_sequences(&self.db);
        let profile_facts = tools::memory_profile(&self.db).conventions.len()
            + tools::memory_profile(&self.db).preferences.len();
        serde_json::to_string(&serde_json::json!({
            "episodes_total": report.total_episodes(),
            "ready": report.ready,
            "pending": report.pending,
            "retry_queued": report.retry_queued,
            "failed_final": report.failed_final,
            "summary_artifacts": report.summary_artifacts,
            "extraction_artifacts": report.extraction_artifacts,
            "workflows": workflows.len(),
            "profile_facts": profile_facts,
        }))
        .unwrap_or_default()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LobsterServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_instructions(
            "Lobster: local, deterministic, per-repo memory for Claude Code",
        )
    }
}

/// Run the MCP server on stdio.
///
/// # Errors
///
/// Returns an error if the server fails to start or encounters a
/// fatal transport error.
pub async fn run_server(
    db: Arc<LobsterDb>,
    grafeo: Arc<GrafeoDB>,
) -> anyhow::Result<()> {
    let server = LobsterServer::new(db, grafeo);
    let transport = rmcp::transport::stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db as lobster_db;

    #[test]
    fn test_server_creates() {
        let db = {
            let (db, _dir) = lobster_db::open_in_memory().unwrap();
            Arc::new(db)
        };
        let grafeo = Arc::new(crate::graph::db::new_in_memory());
        let server = LobsterServer::new(db, grafeo);
        let info = server.get_info();
        assert!(info.instructions.is_some());
    }

    #[derive(Debug, Clone, Default)]
    struct TestClient;

    impl rmcp::ClientHandler for TestClient {
        fn get_info(&self) -> rmcp::model::ClientInfo {
            rmcp::model::ClientInfo::default()
        }
    }

    #[tokio::test]
    async fn test_server_handshake() {
        let db = {
            let (db, _dir) = lobster_db::open_in_memory().unwrap();
            Arc::new(db)
        };
        let grafeo = Arc::new(crate::graph::db::new_in_memory());
        let server = LobsterServer::new(db, grafeo);

        // Use duplex transport to test the full handshake
        let (server_transport, client_transport) = tokio::io::duplex(4096);

        let server_handle = tokio::spawn(async move {
            server
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });

        // Client side: send initialize
        let client = TestClient.serve(client_transport).await.unwrap();

        // List tools
        let tools = client.list_all_tools().await.unwrap();

        assert_eq!(tools.len(), 7);

        let names: Vec<String> =
            tools.iter().map(|t| t.name.to_string()).collect();
        let has = |n: &str| names.iter().any(|s| s == n);
        assert!(has("memory_context"));
        assert!(has("memory_search"));
        assert!(has("memory_status"));
        assert!(has("memory_recent"));
        assert!(has("memory_decisions"));
        assert!(has("memory_neighbors"));
        assert!(has("memory_profile"));

        // Call a tool
        let result = client
            .call_tool(rmcp::model::CallToolRequestParams::new("memory_status"))
            .await
            .unwrap();
        assert!(!result.content.is_empty());

        client.cancel().await.unwrap();
        let _ = server_handle.await;
    }
}

mod server;
mod session;

use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing to stderr (stdout is used for MCP stdio transport)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting terminal-execute MCP server");

    let service = server::TerminalServer::new();
    let server = service.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Failed to start server: {}", e);
    })?;

    tracing::info!("Server initialized, waiting for requests");
    server.waiting().await?;

    tracing::info!("Server shutting down");
    Ok(())
}

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::session::SessionManager;

#[derive(Clone)]
pub struct TerminalServer {
    session_manager: Arc<SessionManager>,
    tool_router: ToolRouter<Self>,
}

impl TerminalServer {
    pub fn new() -> Self {
        Self {
            session_manager: Arc::new(SessionManager::new()),
            tool_router: Self::tool_router(),
        }
    }
}

// -- Tool parameter types --

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExecuteParams {
    /// Shell command to execute (e.g. "ls -la", "cargo build")
    pub command: String,
    /// Working directory. Defaults to server's cwd
    pub cwd: Option<String>,
    /// Timeout in seconds. Default: 300 (5 min)
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSessionParams {
    /// Working directory for the shell
    pub cwd: Option<String>,
    /// Shell to use (e.g. "/bin/bash", "/bin/zsh"). Defaults to $SHELL
    pub shell: Option<String>,
    /// Project name for tagging/filtering
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendInputParams {
    /// Session ID returned by create_session
    pub session_id: String,
    /// Text to send to the terminal (include \\n for Enter)
    pub input: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadOutputParams {
    /// Session ID returned by create_session
    pub session_id: String,
    /// Max number of lines to return (from the end). Omit for all
    pub lines: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloseSessionParams {
    /// Session ID returned by create_session
    pub session_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListSessionsParams {
    /// Filter by project name
    pub project: Option<String>,
}

#[tool_router]
impl TerminalServer {
    #[tool(description = "Execute a shell command synchronously. Waits for completion and returns stdout and exit code. Use for simple one-off commands.")]
    async fn execute(&self, Parameters(params): Parameters<ExecuteParams>) -> String {
        tracing::info!(command = %params.command, cwd = ?params.cwd, "Executing command");

        match self.session_manager.execute(&params.command, params.cwd, params.timeout_secs) {
            Ok(result) => {
                format!(
                    "Exit code: {}\n\n{}",
                    result.exit_code,
                    result.stdout
                )
            }
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[tool(description = "Create a new interactive terminal session with a PTY. Returns a session_id for subsequent send_input/read_output calls. Use for long-running or interactive commands.")]
    async fn create_session(&self, Parameters(params): Parameters<CreateSessionParams>) -> String {
        tracing::info!(cwd = ?params.cwd, project = ?params.project, "Creating session");

        match self.session_manager.create_session(params.cwd, params.shell, params.project) {
            Ok(session_id) => {
                serde_json::json!({ "session_id": session_id }).to_string()
            }
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[tool(description = "Send input text to an interactive terminal session. Include newline character to submit commands.")]
    async fn send_input(&self, Parameters(params): Parameters<SendInputParams>) -> String {
        tracing::info!(session_id = %params.session_id, "Sending input");

        match self.session_manager.send_input(&params.session_id, &params.input) {
            Ok(()) => "Input sent".to_string(),
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[tool(description = "Read accumulated output from a terminal session. This is a destructive read - the buffer is cleared after reading. Returns the output text and whether the session is still alive.")]
    async fn read_output(&self, Parameters(params): Parameters<ReadOutputParams>) -> String {
        tracing::info!(session_id = %params.session_id, "Reading output");

        match self.session_manager.read_output(&params.session_id, params.lines) {
            Ok((output, is_alive)) => {
                format!(
                    "alive: {}\n\n{}",
                    is_alive,
                    if output.is_empty() { "(no new output)" } else { &output }
                )
            }
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[tool(description = "Close and terminate a terminal session. The PTY and child process are killed.")]
    async fn close_session(&self, Parameters(params): Parameters<CloseSessionParams>) -> String {
        tracing::info!(session_id = %params.session_id, "Closing session");

        match self.session_manager.close_session(&params.session_id) {
            Ok(()) => "Session closed".to_string(),
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[tool(description = "List all active terminal sessions. Optionally filter by project name.")]
    async fn list_sessions(&self, Parameters(params): Parameters<ListSessionsParams>) -> String {
        let sessions = self.session_manager.list_sessions(params.project.as_deref());

        if sessions.is_empty() {
            "No active sessions".to_string()
        } else {
            serde_json::to_string_pretty(&sessions).unwrap_or_else(|e| format!("ERROR: {}", e))
        }
    }
}

#[tool_handler]
impl ServerHandler for TerminalServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Terminal session manager. Use 'execute' for simple one-off commands, \
                 or create_session/send_input/read_output/close_session for interactive terminals."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

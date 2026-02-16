# mcp-terminal

A Rust-based [MCP](https://modelcontextprotocol.io/) server for terminal session management with full PTY support. Built as a reliable replacement for basic command execution tools, providing persistent interactive sessions.

## Features

- **`execute`** — Run one-off commands with stdout/stderr capture and exit code
- **`create_session`** — Create persistent PTY sessions (interactive shells, REPLs, long-running processes)
- **`send_input`** — Send keystrokes/commands to a running session
- **`read_output`** — Read buffered output from a session (ring buffer, non-blocking)
- **`close_session`** — Terminate a session and clean up resources
- **`list_sessions`** — List all active sessions with metadata

## Key Design

- **PTY-based** via [`portable-pty`](https://crates.io/crates/portable-pty) — real terminal emulation, not just pipes
- **Ring buffer output** — efficient memory usage, configurable buffer size
- **Project tagging** — optionally tag sessions for organization
- **MCP stdio transport** via [`rmcp`](https://crates.io/crates/rmcp) SDK

## Building

```bash
cargo build --release
```

The binary will be at `target/release/mcp-terminal`.

## Configuration

Add to your MCP client config (e.g. Gemini Code Assist, Claude Desktop):

```json
{
  "mcpServers": {
    "terminal": {
      "command": "/path/to/mcp-terminal"
    }
  }
}
```

## Environment Variables

- `RUST_LOG` — Controls log verbosity (default: `info`). Logs go to stderr.

## License

This project is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-or-later).

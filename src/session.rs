use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

/// Maximum output buffer size per session (1 MB)
const MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// A terminal session backed by a PTY
struct SessionInner {
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    is_alive: Arc<Mutex<bool>>,
    _reader_handle: std::thread::JoinHandle<()>,
}

/// Public session metadata
#[derive(Clone, serde::Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub project: Option<String>,
    pub cwd: String,
    pub is_alive: bool,
    pub created_at: DateTime<Utc>,
}

/// Full session: inner PTY state + metadata
struct Session {
    inner: SessionInner,
    project: Option<String>,
    cwd: String,
    created_at: DateTime<Utc>,
}

/// Result of a synchronous command execution
#[derive(serde::Serialize)]
pub struct ExecResult {
    pub stdout: String,
    pub exit_code: u32,
}

/// Manages all terminal sessions
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Session>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Create a new interactive terminal session
    pub fn create_session(
        &self,
        cwd: Option<String>,
        shell: Option<String>,
        project: Option<String>,
    ) -> Result<String, String> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        let shell_cmd = shell.unwrap_or_else(|| {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
        });

        let working_dir = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string())
        });

        let mut cmd = CommandBuilder::new(&shell_cmd);
        cmd.cwd(&working_dir);

        // Spawn the shell in the slave PTY
        let _child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn shell: {}", e))?;

        // Drop the slave — we only need the master side
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get PTY reader: {}", e))?;

        let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let is_alive = Arc::new(Mutex::new(true));

        // Spawn a background thread to continuously read PTY output
        let output_clone = Arc::clone(&output);
        let alive_clone = Arc::clone(&is_alive);
        let reader_handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF — process exited
                        *alive_clone.lock().unwrap() = false;
                        break;
                    }
                    Ok(n) => {
                        let mut output = output_clone.lock().unwrap();
                        output.extend_from_slice(&buf[..n]);
                        // Trim if over max size — keep the tail
                        if output.len() > MAX_BUFFER_SIZE {
                            let drain_to = output.len() - MAX_BUFFER_SIZE;
                            output.drain(..drain_to);
                        }
                    }
                    Err(_) => {
                        *alive_clone.lock().unwrap() = false;
                        break;
                    }
                }
            }
        });

        let session_id = Uuid::new_v4().to_string();

        let session = Session {
            inner: SessionInner {
                _master: pair.master,
                writer,
                output,
                is_alive,
                _reader_handle: reader_handle,
            },
            project,
            cwd: working_dir,
            created_at: Utc::now(),
        };

        self.sessions
            .lock()
            .unwrap()
            .insert(session_id.clone(), session);

        Ok(session_id)
    }

    /// Send input text to a session
    pub fn send_input(&self, session_id: &str, input: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        session
            .inner
            .writer
            .write_all(input.as_bytes())
            .map_err(|e| format!("Failed to write to PTY: {}", e))?;

        session
            .inner
            .writer
            .flush()
            .map_err(|e| format!("Failed to flush PTY: {}", e))?;

        Ok(())
    }

    /// Read and drain accumulated output from a session
    pub fn read_output(&self, session_id: &str, max_lines: Option<u32>) -> Result<(String, bool), String> {
        let sessions = self.sessions.lock().unwrap();
        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        let mut output_buf = session.inner.output.lock().unwrap();
        let is_alive = *session.inner.is_alive.lock().unwrap();

        let raw = std::mem::take(&mut *output_buf);
        let text = String::from_utf8_lossy(&raw).to_string();

        // Strip ANSI escape sequences for cleaner output
        let cleaned = strip_ansi_escapes(&text);

        // Optionally limit lines
        let result = if let Some(max) = max_lines {
            let lines: Vec<&str> = cleaned.lines().collect();
            let start = lines.len().saturating_sub(max as usize);
            lines[start..].join("\n")
        } else {
            cleaned
        };

        Ok((result, is_alive))
    }

    /// Close and remove a session
    pub fn close_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.lock().unwrap();
        let _session = sessions
            .remove(session_id)
            .ok_or_else(|| format!("Session {} not found", session_id))?;

        // Dropping the session will close the PTY master, which kills the child process
        Ok(())
    }

    /// List all sessions, optionally filtered by project
    pub fn list_sessions(&self, project: Option<&str>) -> Vec<SessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .filter(|(_, s)| {
                if let Some(proj) = project {
                    s.project.as_deref() == Some(proj)
                } else {
                    true
                }
            })
            .map(|(id, s)| SessionInfo {
                session_id: id.clone(),
                project: s.project.clone(),
                cwd: s.cwd.clone(),
                is_alive: *s.inner.is_alive.lock().unwrap(),
                created_at: s.created_at,
            })
            .collect()
    }

    /// Execute a command synchronously — create a temporary PTY, run the command,
    /// wait for completion, return output
    pub fn execute(
        &self,
        command: &str,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<ExecResult, String> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        let working_dir = cwd.unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/tmp".to_string())
        });

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(&working_dir);

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn command: {}", e))?;

        drop(pair.slave);

        // Read output in a background thread
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get reader: {}", e))?;

        let output: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let output_clone = Arc::clone(&output);

        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut out = output_clone.lock().unwrap();
                        out.extend_from_slice(&buf[..n]);
                        // Hard cap at 2 MB for synchronous execution
                        if out.len() > 2 * 1024 * 1024 {
                            let drain_to = out.len() - 2 * 1024 * 1024;
                            out.drain(..drain_to);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Wait for the child with optional timeout
        let timeout = Duration::from_secs(timeout_secs.unwrap_or(300));
        let exit_code = match wait_with_timeout(&mut child, timeout) {
            Ok(status) => status.exit_code(),
            Err(e) => {
                // Try to kill on timeout
                let _ = child.kill();
                return Err(format!("Command timed out after {}s: {}", timeout.as_secs(), e));
            }
        };

        // Wait for reader thread to finish
        let _ = reader_thread.join();

        // Drop the master to ensure reader thread exits
        drop(pair.master);

        let raw_output = output.lock().unwrap();
        let stdout = String::from_utf8_lossy(&raw_output).to_string();
        let cleaned = strip_ansi_escapes(&stdout);

        Ok(ExecResult {
            stdout: cleaned,
            exit_code,
        })
    }
}

/// Wait for child process with timeout using polling
fn wait_with_timeout(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    timeout: Duration,
) -> Result<portable_pty::ExitStatus, String> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(50);

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return Err("Timeout".to_string());
                }
                std::thread::sleep(poll_interval);
            }
            Err(e) => return Err(format!("Wait error: {}", e)),
        }
    }
}

/// Strip ANSI escape sequences from text
fn strip_ansi_escapes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // ESC sequence
            if let Some(&next) = chars.peek() {
                if next == '[' {
                    chars.next(); // consume '['
                    // Read until we hit a letter (the final byte of the sequence)
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c.is_ascii_alphabetic() || c == 'H' || c == 'J' || c == 'K' {
                            break;
                        }
                    }
                } else if next == ']' {
                    chars.next(); // consume ']'
                    // OSC sequence — read until BEL (\x07) or ST (\x1b\\)
                    while let Some(&c) = chars.peek() {
                        chars.next();
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                } else if next == '(' || next == ')' {
                    chars.next();
                    chars.next(); // skip charset designation
                }
            }
        } else if ch == '\r' {
            // Skip carriage returns — common in PTY output
            continue;
        } else {
            result.push(ch);
        }
    }

    result
}

//! The wrapped rust-analyzer child: the editor's real Rust language server.
//!
//! In proxy mode typeweld-ls is the only Rust language server the editor
//! runs; this module owns the actual rust-analyzer process behind it. All
//! traffic is forwarded verbatim by [`crate::proxy`]; this module only spawns
//! the child, resolves the binary, and replays session state after a crash.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use lsp_server::{Message, Notification, Request, RequestId};

use crate::convert::path_to_uri;

/// How quickly consecutive respawns may happen; faster failures degrade the
/// session instead of spinning.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(10);

/// Resolves the rust-analyzer binary to wrap.
///
/// Order: explicit override (CLI flag), `TYPEWELD_RUST_ANALYZER`, `PATH`,
/// `rustup which rust-analyzer`. Setting the environment variable to `off`
/// disables the child entirely.
pub fn resolve_binary(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("TYPEWELD_RUST_ANALYZER") {
        if value == "off" {
            return None;
        }
        return Some(PathBuf::from(value));
    }
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    if let Some(path) = find_in_path("rust-analyzer") {
        return Some(path);
    }
    let output = Command::new("rustup")
        .args(["which", "rust-analyzer"])
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8(output.stdout).ok()?;
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        let exe = dir.join(format!("{name}.exe"));
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

/// One running rust-analyzer child with its reader channel.
pub struct RaChild {
    binary: PathBuf,
    root: PathBuf,
    process: Child,
    stdin: ChildStdin,
    /// Messages from the child; disconnects when the child exits.
    pub incoming: Receiver<Message>,
    last_spawn: Instant,
}

impl RaChild {
    /// Spawns the child and performs the initialize handshake with the
    /// editor's own (raw) initialize params, so the child negotiates exactly
    /// what the editor asked for. Returns the child and the raw
    /// `InitializeResult` to relay to the editor.
    pub fn spawn(
        binary: &Path,
        root: &Path,
        initialize_params: &serde_json::Value,
    ) -> Result<(Self, serde_json::Value), String> {
        let mut process = Command::new(binary)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to spawn `{}`: {error}", binary.display()))?;
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| "rust-analyzer stdin unavailable".to_owned())?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| "rust-analyzer stdout unavailable".to_owned())?;

        let (sender, incoming) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(message)) = Message::read(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });

        let mut child = Self {
            binary: binary.to_path_buf(),
            root: root.to_path_buf(),
            process,
            stdin,
            incoming,
            last_spawn: Instant::now(),
        };

        // The child must see our process id, not the editor's, so it exits
        // with us instead of outliving a dead proxy.
        let mut params = initialize_params.clone();
        if let Some(object) = params.as_object_mut() {
            object.insert(
                "processId".to_owned(),
                serde_json::json!(std::process::id()),
            );
        }
        let init_id = RequestId::from("typeweld:init".to_owned());
        child.send(&Message::Request(Request::new(
            init_id.clone(),
            "initialize".to_owned(),
            params,
        )))?;
        let result = child.wait_for_response(&init_id, Duration::from_mins(1))?;
        child.send(&Message::Notification(Notification::new(
            "initialized".to_owned(),
            serde_json::json!({}),
        )))?;
        Ok((child, result))
    }

    /// Respawns after a crash, replaying the handshake and the open
    /// documents. Refuses to respawn while the previous spawn is younger than
    /// the cooldown, so a crash-looping child degrades instead of spinning.
    pub fn respawn(
        &self,
        initialize_params: &serde_json::Value,
        open_documents: &[(PathBuf, String, i32)],
    ) -> Result<Self, String> {
        if self.last_spawn.elapsed() < RESPAWN_COOLDOWN {
            return Err("rust-analyzer is crash-looping; not respawning yet".to_owned());
        }
        let (mut child, _) = Self::spawn(&self.binary, &self.root, initialize_params)?;
        for (path, text, version) in open_documents {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "languageId": "rust",
                    "version": version,
                    "text": text,
                },
            });
            child.send(&Message::Notification(Notification::new(
                "textDocument/didOpen".to_owned(),
                params,
            )))?;
        }
        Ok(child)
    }

    /// Writes one message to the child.
    pub fn send(&mut self, message: &Message) -> Result<(), String> {
        message
            .write(&mut self.stdin)
            .map_err(|error| format!("rust-analyzer write failed: {error}"))
    }

    /// Drains the channel until the response with `id` arrives, answering
    /// child-to-server requests with trivial defaults in the meantime (only
    /// used during the handshake, before the editor can answer for real).
    fn wait_for_response(
        &mut self,
        id: &RequestId,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self.incoming.recv_timeout(remaining).map_err(|_| {
                format!("rust-analyzer did not answer within {}s", timeout.as_secs())
            })?;
            match message {
                Message::Response(response) if &response.id == id => {
                    if let Some(error) = response.error {
                        return Err(format!(
                            "rust-analyzer initialize failed: {}",
                            error.message
                        ));
                    }
                    return Ok(response.result.unwrap_or(serde_json::Value::Null));
                }
                Message::Response(_) | Message::Notification(_) => {}
                Message::Request(request) => {
                    let result = if request.method == "workspace/configuration" {
                        let items = request
                            .params
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or(1, Vec::len);
                        serde_json::Value::Array(vec![serde_json::Value::Null; items])
                    } else {
                        serde_json::Value::Null
                    };
                    self.send(&Message::Response(lsp_server::Response::new_ok(
                        request.id, result,
                    )))?;
                }
            }
        }
    }
}

impl Drop for RaChild {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

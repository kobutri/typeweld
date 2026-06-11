//! Private typescript-language-server child used as a semantic query backend.
//!
//! The server owns a dedicated typescript-language-server rooted at the
//! workspace so renames of generated interface properties (struct fields,
//! endpoint args) also update the user TypeScript property accesses the
//! contract cannot see. The child is never exposed to the editor: it is
//! spawned lazily on the first query and killed and respawned transparently
//! after a crash or timeout. Every failure is reported as an `Err` so callers
//! can degrade to contract-based plans.
//!
//! Renames are anchored at a property declaration inside a generated file;
//! tsserver finds the package `tsconfig.json` next to it and loads the
//! project spanning the bindings and the app sources. Until that project has
//! finished loading, tsserver serves partial-semantic answers covering only
//! open files, so the first query is held until the project-loading progress
//! reports `end` (with a short fallback when no progress arrives at all).

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::WorkspaceEdit;

use crate::convert::path_to_uri;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for the first project-loading progress signal after a
/// didOpen before assuming none will come and proceeding anyway.
const PROGRESS_GRACE: Duration = Duration::from_secs(3);

/// Supervisor for the private typescript-language-server child process.
pub struct TsBackend {
    root: PathBuf,
    binary: PathBuf,
    child: Option<ChildHandle>,
}

impl TsBackend {
    /// Creates an idle supervisor; nothing is spawned until the first query.
    ///
    /// The binary resolves to `TYPEWELD_TSLS` when set, then
    /// `typescript-language-server` from `PATH`. `TYPEWELD_TSSERVER`
    /// optionally pins the tsserver module the child should load.
    pub fn new(root: PathBuf) -> Self {
        let binary = std::env::var_os("TYPEWELD_TSLS").map_or_else(
            || PathBuf::from("typescript-language-server"),
            PathBuf::from,
        );
        Self {
            root,
            binary,
            child: None,
        }
    }

    /// The per-request timeout. `TYPEWELD_TSLS_TIMEOUT_SECS` overrides it.
    pub fn request_timeout() -> Duration {
        std::env::var("TYPEWELD_TSLS_TIMEOUT_SECS")
            .ok()
            .and_then(|seconds| seconds.parse().ok())
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
    }

    /// Queries the child for a rename at a UTF-16 position inside the
    /// generated `anchor_file` (read from disk and opened on demand).
    ///
    /// Spawns (or respawns) the child first when necessary. Any failure —
    /// spawn, crash, timeout, protocol error — kills the child so the next
    /// call starts fresh, and is reported as `Err`.
    pub fn rename(
        &mut self,
        anchor_file: &Path,
        line: u32,
        character: u32,
        new_name: &str,
        timeout: Duration,
    ) -> Result<Option<WorkspaceEdit>, String> {
        self.ensure_child(timeout)?;
        let child = self.child.as_mut().expect("child ensured above");
        let outcome = child.rename(anchor_file, line, character, new_name, timeout);
        if outcome.is_err() {
            self.child = None;
        }
        outcome
    }

    /// Spawns and initializes a child unless a live one is already running.
    fn ensure_child(&mut self, timeout: Duration) -> Result<(), String> {
        let exited = self
            .child
            .as_mut()
            .is_some_and(|child| !matches!(child.process.try_wait(), Ok(None)));
        if exited {
            self.child = None;
        }
        if self.child.is_none() {
            self.child = Some(ChildHandle::spawn(&self.root, &self.binary, timeout)?);
        }
        Ok(())
    }
}

/// One running typescript-language-server: process, pipes, protocol state.
struct ChildHandle {
    process: Child,
    stdin: ChildStdin,
    /// Messages the reader thread pulled off the child's stdout; the channel
    /// disconnects when the child exits.
    incoming: Receiver<Message>,
    next_id: i32,
    /// Last text and version sent for each opened anchor file. Anchors are
    /// generated files, so disk is the source of truth: regeneration rewrites
    /// them in place and the next query sends a didChange with fresh contents.
    opened: HashMap<PathBuf, (String, i32)>,
    /// Whether the first query already held for project loading.
    waited_for_load: bool,
}

impl ChildHandle {
    /// Spawns the child and runs the initialize handshake, declaring
    /// `window.workDoneProgress` so the child reports project loading.
    fn spawn(root: &Path, binary: &Path, timeout: Duration) -> Result<Self, String> {
        let mut process = Command::new(binary)
            .arg("--stdio")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to spawn `{}`: {error}", binary.display()))?;
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| "typescript-language-server stdin unavailable".to_owned())?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| "typescript-language-server stdout unavailable".to_owned())?;

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
            process,
            stdin,
            incoming,
            next_id: 0,
            opened: HashMap::new(),
            waited_for_load: false,
        };
        let root_uri = path_to_uri(root);
        let mut params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                // Report project-loading progress so the first query can be
                // held until the configured project is loaded (before that,
                // tsserver answers from partial semantics over open files).
                "window": { "workDoneProgress": true },
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "typeweld" }],
        });
        if let Some(tsserver) = std::env::var_os("TYPEWELD_TSSERVER") {
            params["initializationOptions"] = serde_json::json!({
                "tsserver": { "path": tsserver.to_string_lossy() },
            });
        }
        child.request("initialize", params, timeout)?;
        child.notify("initialized", serde_json::json!({}))?;
        Ok(child)
    }

    /// Performs one rename query at the anchor position.
    fn rename(
        &mut self,
        anchor_file: &Path,
        line: u32,
        character: u32,
        new_name: &str,
        timeout: Duration,
    ) -> Result<Option<WorkspaceEdit>, String> {
        self.sync_anchor(anchor_file)?;
        if !self.waited_for_load {
            self.waited_for_load = true;
            self.wait_for_project_load(timeout)?;
        }
        let params = serde_json::json!({
            "textDocument": { "uri": path_to_uri(anchor_file) },
            "position": { "line": line, "character": character },
            "newName": new_name,
        });
        let value = self.request("textDocument/rename", params, timeout)?;
        serde_json::from_value(value).map_err(|error| {
            format!("typescript-language-server returned an invalid edit: {error}")
        })
    }

    /// Opens the anchor file from disk, or refreshes it when regeneration
    /// rewrote the file since the last query.
    fn sync_anchor(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        match self.opened.get(path) {
            Some((sent, _)) if *sent == text => Ok(()),
            Some((_, version)) => {
                let version = version + 1;
                let params = serde_json::json!({
                    "textDocument": { "uri": path_to_uri(path), "version": version },
                    "contentChanges": [{ "text": text }],
                });
                self.opened.insert(path.to_path_buf(), (text, version));
                self.notify("textDocument/didChange", params)
            }
            None => {
                let params = serde_json::json!({
                    "textDocument": {
                        "uri": path_to_uri(path),
                        "languageId": "typescript",
                        "version": 1,
                        "text": text,
                    },
                });
                self.opened.insert(path.to_path_buf(), (text, 1));
                self.notify("textDocument/didOpen", params)
            }
        }
    }

    /// Blocks until the project-loading progress reports `end`, answering
    /// child requests in the meantime. When no progress signal arrives within
    /// a short grace period the wait is abandoned (older servers, projects
    /// already loaded); callers verify by retrying empty answers once.
    fn wait_for_project_load(&mut self, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        let overall = started + timeout;
        let mut saw_progress = false;
        loop {
            let deadline = if saw_progress {
                overall
            } else {
                (started + PROGRESS_GRACE).min(overall)
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.incoming.recv_timeout(remaining) {
                Ok(Message::Request(request)) => {
                    saw_progress |= request.method == "window/workDoneProgress/create";
                    self.answer(request)?;
                }
                Ok(Message::Notification(notification)) if notification.method == "$/progress" => {
                    saw_progress = true;
                    let ended = notification
                        .params
                        .get("value")
                        .and_then(|value| value.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        == Some("end");
                    if ended {
                        return Ok(());
                    }
                }
                Ok(Message::Notification(_) | Message::Response(_)) => {}
                // No signal (or no `end`) in time: proceed anyway; partial
                // answers are caught by the caller's retry.
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("typescript-language-server exited".to_owned());
                }
            }
        }
    }

    fn send(&mut self, message: &Message) -> Result<(), String> {
        message
            .write(&mut self.stdin)
            .map_err(|error| format!("typescript-language-server write failed: {error}"))
    }

    fn notify(&mut self, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.send(&Message::Notification(Notification::new(
            method.to_owned(),
            params,
        )))
    }

    /// Sends one request and waits for its response, draining (and minimally
    /// answering) everything else the child produces in the meantime.
    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(&Message::Request(Request::new(
            id.clone(),
            method.to_owned(),
            params,
        )))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .incoming
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => format!(
                        "typescript-language-server did not answer `{method}` within {}s",
                        timeout.as_secs()
                    ),
                    RecvTimeoutError::Disconnected => {
                        "typescript-language-server exited".to_owned()
                    }
                })?;
            match message {
                Message::Response(response) if response.id == id => {
                    if let Some(error) = response.error {
                        return Err(format!(
                            "typescript-language-server `{method}`: {}",
                            error.message
                        ));
                    }
                    return Ok(response.result.unwrap_or(serde_json::Value::Null));
                }
                // Stale responses and server notifications (progress, logs,
                // diagnostics) are irrelevant to the query.
                Message::Response(_) | Message::Notification(_) => {}
                Message::Request(request) => self.answer(request)?,
            }
        }
    }

    /// Answers child-to-server requests with trivial defaults so the child
    /// never stalls waiting on us: `workspace/configuration` gets one `null`
    /// per item, everything else (workDoneProgress/create, registrations)
    /// gets a bare `null`.
    fn answer(&mut self, request: Request) -> Result<(), String> {
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
        self.send(&Message::Response(Response::new_ok(request.id, result)))
    }
}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

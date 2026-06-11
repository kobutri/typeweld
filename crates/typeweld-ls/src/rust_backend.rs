//! Private rust-analyzer child process used as a semantic query backend.
//!
//! The server owns a dedicated rust-analyzer over the workspace so renames of
//! Rust-declared API symbols also update the call sites the contract cannot
//! see. The child is never exposed to the editor: it is spawned lazily on the
//! first query, mirrors the server's open Rust documents, and is killed and
//! respawned transparently after a crash or timeout. Every failure is
//! reported as an `Err` so callers can degrade to contract-based plans.

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use lsp_server::{Message, Notification, Request, RequestId, Response};
use lsp_types::{Position, WorkspaceEdit};

use crate::convert::path_to_uri;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Supervisor for the private rust-analyzer child process.
pub struct RustBackend {
    root: PathBuf,
    binary: PathBuf,
    /// Mirrors of the open Rust documents, keyed by absolute path. Queued
    /// here even while no child runs, and replayed on every (re)spawn so the
    /// child always sees unsaved edits before the first request.
    documents: HashMap<PathBuf, Document>,
    child: Option<ChildHandle>,
}

struct Document {
    text: String,
    version: i32,
}

impl RustBackend {
    /// Creates an idle supervisor; nothing is spawned until the first query.
    ///
    /// The binary resolves to `TYPEWELD_RUST_ANALYZER` when set, then the
    /// explicit override, then `rust-analyzer` from `PATH`.
    pub fn new(root: PathBuf, binary: Option<PathBuf>) -> Self {
        let binary = std::env::var_os("TYPEWELD_RUST_ANALYZER")
            .map(PathBuf::from)
            .or(binary)
            .unwrap_or_else(|| PathBuf::from("rust-analyzer"));
        Self {
            root,
            binary,
            documents: HashMap::new(),
            child: None,
        }
    }

    /// The per-request timeout: generous by default because rust-analyzer
    /// holds answers until indexing finishes. `TYPEWELD_RA_TIMEOUT_SECS`
    /// overrides it.
    pub fn request_timeout() -> Duration {
        std::env::var("TYPEWELD_RA_TIMEOUT_SECS")
            .ok()
            .and_then(|seconds| seconds.parse().ok())
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs)
    }

    /// Mirrors the current overlay of an open Rust document into the child;
    /// queued for the next spawn while no child runs.
    pub fn sync_document(&mut self, path: &Path, text: &str, version: i32) {
        let document = Document {
            text: text.to_owned(),
            version,
        };
        if let Some(child) = &mut self.child {
            if child.sync_document(path, &document).is_err() {
                self.child = None;
            }
        }
        self.documents.insert(path.to_path_buf(), document);
    }

    /// Drops the mirror of a closed document; the child falls back to disk.
    pub fn close_document(&mut self, path: &Path) {
        if self.documents.remove(path).is_none() {
            return;
        }
        let Some(child) = &mut self.child else {
            return;
        };
        if child.opened.remove(path) {
            let params = serde_json::json!({
                "textDocument": { "uri": path_to_uri(path) },
            });
            if child.notify("textDocument/didClose", params).is_err() {
                self.child = None;
            }
        }
    }

    /// Queries the child for a semantic rename at a UTF-16 `position`.
    ///
    /// Spawns (or respawns) the child first when necessary. Any failure —
    /// spawn, crash, timeout, protocol error — kills the child so the next
    /// call starts fresh, and is reported as `Err`.
    pub fn rename(
        &mut self,
        path: &Path,
        position: Position,
        new_name: &str,
        timeout: Duration,
    ) -> Result<Option<WorkspaceEdit>, String> {
        self.ensure_child(timeout)?;
        let child = self.child.as_mut().expect("child ensured above");
        let params = serde_json::json!({
            "textDocument": { "uri": path_to_uri(path) },
            "position": position,
            "newName": new_name,
        });
        match child.request("textDocument/rename", params, timeout) {
            Ok(value) => serde_json::from_value(value)
                .map_err(|error| format!("rust-analyzer returned an invalid edit: {error}")),
            Err(message) => {
                self.child = None;
                Err(message)
            }
        }
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
            self.child = Some(ChildHandle::spawn(
                &self.root,
                &self.binary,
                &self.documents,
                timeout,
            )?);
        }
        Ok(())
    }
}

/// One running rust-analyzer: process, pipes, and protocol state.
struct ChildHandle {
    process: Child,
    stdin: ChildStdin,
    /// Messages the reader thread pulled off the child's stdout; the channel
    /// disconnects when the child exits.
    incoming: Receiver<Message>,
    next_id: i32,
    /// Documents the child has received a didOpen for.
    opened: HashSet<PathBuf>,
}

impl ChildHandle {
    /// Spawns the child, runs the initialize handshake (UTF-16 positions),
    /// and replays the queued document mirrors.
    fn spawn(
        root: &Path,
        binary: &Path,
        documents: &HashMap<PathBuf, Document>,
        timeout: Duration,
    ) -> Result<Self, String> {
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
            process,
            stdin,
            incoming,
            next_id: 0,
            opened: HashSet::new(),
        };
        let root_uri = path_to_uri(root);
        let params = serde_json::json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                // Report indexing progress so we can hold queries until the
                // workspace is loaded (early renames find no references).
                "experimental": { "serverStatusNotification": true },
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "typeweld" }],
        });
        child.request("initialize", params, timeout)?;
        child.notify("initialized", serde_json::json!({}))?;
        for (path, document) in documents {
            child.open_document(path, document)?;
        }
        child.wait_until_quiescent(timeout)?;
        Ok(child)
    }

    /// Blocks until the child reports a quiescent (fully indexed) workspace,
    /// answering its requests in the meantime. Queries sent before that point
    /// can come back empty instead of being held.
    fn wait_until_quiescent(&mut self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .incoming
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => format!(
                        "rust-analyzer did not finish indexing within {}s",
                        timeout.as_secs()
                    ),
                    RecvTimeoutError::Disconnected => "rust-analyzer exited".to_owned(),
                })?;
            match message {
                Message::Notification(notification)
                    if notification.method == "experimental/serverStatus" =>
                {
                    let quiescent = notification
                        .params
                        .get("quiescent")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    if quiescent {
                        return Ok(());
                    }
                }
                Message::Notification(_) | Message::Response(_) => {}
                Message::Request(request) => self.answer(request)?,
            }
        }
    }

    fn send(&mut self, message: &Message) -> Result<(), String> {
        message
            .write(&mut self.stdin)
            .map_err(|error| format!("rust-analyzer write failed: {error}"))
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
                        "rust-analyzer did not answer `{method}` within {}s",
                        timeout.as_secs()
                    ),
                    RecvTimeoutError::Disconnected => "rust-analyzer exited".to_owned(),
                })?;
            match message {
                Message::Response(response) if response.id == id => {
                    if let Some(error) = response.error {
                        return Err(format!("rust-analyzer `{method}`: {}", error.message));
                    }
                    return Ok(response.result.unwrap_or(serde_json::Value::Null));
                }
                // Stale responses and server notifications (progress,
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

    /// Forwards one document mirror: didOpen the first time, didChange after.
    fn sync_document(&mut self, path: &Path, document: &Document) -> Result<(), String> {
        if self.opened.contains(path) {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": path_to_uri(path),
                    "version": document.version,
                },
                "contentChanges": [{ "text": document.text }],
            });
            self.notify("textDocument/didChange", params)
        } else {
            self.open_document(path, document)
        }
    }

    fn open_document(&mut self, path: &Path, document: &Document) -> Result<(), String> {
        let params = serde_json::json!({
            "textDocument": {
                "uri": path_to_uri(path),
                "languageId": "rust",
                "version": document.version,
                "text": document.text,
            },
        });
        self.opened.insert(path.to_path_buf());
        self.notify("textDocument/didOpen", params)
    }
}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

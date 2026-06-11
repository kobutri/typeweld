//! The daemon side of the TypeScript-plugin channel.
//!
//! `@typeweld/typescript-plugin` runs inside the user's own tsserver (VS
//! Code's built-in TypeScript, typescript-language-server, vtsls, …) and
//! connects back here over localhost TCP. The hub publishes the discovery
//! file `target/typeweld/ls.json`, pushes contract snapshots to every
//! connected plugin, answers nothing itself — and asks the plugin for the
//! semantic TypeScript answers the contract cannot see (property accesses
//! during a field rename). The plugin reports TypeScript-initiated renames of
//! API symbols back as [`PluginEvent::RenameLanded`]. The VS Code extension
//! can also connect to this channel to save editor documents after internal
//! edits land.
//!
//! Wire format: newline-delimited JSON, authenticated by a session token from
//! the discovery file.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::Receiver;

/// A semantic event reported by a plugin, consumed by the proxy main loop.
#[derive(Debug)]
pub enum PluginEvent {
    /// The editor applied a TypeScript-side rename of an API symbol; the
    /// Rust complement still needs to happen.
    RenameLanded {
        symbol: String,
        new_name: String,
        files: Vec<PathBuf>,
    },
}

/// One rename location reported by the plugin, in UTF-16 code units.
#[derive(Debug, serde::Deserialize)]
pub struct PluginLocation {
    pub file: String,
    pub start: u32,
    pub length: u32,
}

/// The listening hub. Dropping it removes the discovery file.
pub struct PluginHub {
    port_file: PathBuf,
    pub events: Receiver<PluginEvent>,
    shared: Arc<Shared>,
}

struct Shared {
    token: String,
    /// The latest snapshot line, replayed to every plugin that connects.
    snapshot: Mutex<Option<String>>,
    sessions: Mutex<Vec<Session>>,
    next_query: AtomicI64,
}

struct Session {
    kind: SessionKind,
    writer: Arc<Mutex<TcpStream>>,
    pending: Arc<Mutex<HashMap<i64, crossbeam_channel::Sender<serde_json::Value>>>>,
    alive: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SessionKind {
    TypeScript,
    Vscode,
}

impl PluginHub {
    /// Binds the hub and writes the discovery file. Returns `None` when the
    /// socket or the discovery file cannot be created — the server then runs
    /// without TypeScript plugin support.
    pub fn start(root: &Path) -> Option<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = fresh_token();

        let dir = root.join("target/typeweld");
        std::fs::create_dir_all(&dir).ok()?;
        let port_file = dir.join("ls.json");
        let contents = serde_json::json!({
            "port": port,
            "token": token,
            "pid": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
        });
        std::fs::write(&port_file, contents.to_string()).ok()?;

        let (event_sender, events) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared {
            token,
            snapshot: Mutex::new(None),
            sessions: Mutex::new(Vec::new()),
            next_query: AtomicI64::new(0),
        });

        let accept_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let shared = Arc::clone(&accept_shared);
                let events = event_sender.clone();
                std::thread::spawn(move || serve_plugin(stream, &shared, &events));
            }
        });

        Some(Self {
            port_file,
            events,
            shared,
        })
    }

    /// Replaces the snapshot and pushes it to every connected plugin.
    pub fn set_snapshot(&self, snapshot: &serde_json::Value) {
        let line = snapshot.to_string();
        *self.shared.snapshot.lock().expect("snapshot lock") = Some(line.clone());
        let sessions = self.shared.sessions.lock().expect("sessions lock");
        for session in sessions.iter() {
            if session.kind == SessionKind::TypeScript && session.alive.load(Ordering::Relaxed) {
                let _ = write_line(&session.writer, &line);
            }
        }
    }

    /// Asks connected editor integrations to save these file documents.
    pub fn save_documents(&self, files: &[PathBuf]) {
        if files.is_empty() {
            return;
        }
        let files: Vec<String> = files
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();
        let line = serde_json::json!({
            "method": "saveDocuments",
            "files": files,
        })
        .to_string();
        let sessions = self.shared.sessions.lock().expect("sessions lock");
        for session in sessions.iter() {
            if session.kind == SessionKind::Vscode && session.alive.load(Ordering::Relaxed) {
                let _ = write_line(&session.writer, &line);
            }
        }
    }

    /// Asks a connected plugin for the semantic rename locations of the
    /// property declared at `file`/`offset` (UTF-16). Fails when no plugin is
    /// connected or none answers within `timeout`.
    pub fn query_rename_locations(
        &self,
        file: &Path,
        offset: u32,
        timeout: Duration,
    ) -> Result<Vec<PluginLocation>, String> {
        let id = self.shared.next_query.fetch_add(1, Ordering::Relaxed);
        let query = serde_json::json!({
            "method": "query",
            "id": id,
            "kind": "renameLocations",
            "file": file.to_string_lossy(),
            "position": offset,
        })
        .to_string();

        let receiver = {
            let sessions = self.shared.sessions.lock().expect("sessions lock");
            let session = sessions
                .iter()
                .rev()
                .find(|session| {
                    session.kind == SessionKind::TypeScript && session.alive.load(Ordering::Relaxed)
                })
                .ok_or_else(|| "no TypeScript plugin connected".to_owned())?;
            let (sender, receiver) = crossbeam_channel::bounded(1);
            session
                .pending
                .lock()
                .expect("pending lock")
                .insert(id, sender);
            write_line(&session.writer, &query)
                .map_err(|error| format!("plugin write failed: {error}"))?;
            receiver
        };

        let value = receiver
            .recv_timeout(timeout)
            .map_err(|_| "TypeScript plugin did not answer in time".to_owned())?;
        serde_json::from_value::<Vec<PluginLocation>>(value.clone())
            .map_err(|error| format!("plugin returned invalid locations: {error}"))
    }
}

impl Drop for PluginHub {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.port_file);
    }
}

/// Handles one plugin connection: hello handshake, snapshot replay, then a
/// loop routing query responses and events.
fn serve_plugin(
    stream: TcpStream,
    shared: &Shared,
    events: &crossbeam_channel::Sender<PluginEvent>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(stream));
    let mut reader = BufReader::new(reader_stream);

    let mut hello = String::new();
    if reader.read_line(&mut hello).is_err() {
        return;
    }
    let Ok(hello) = serde_json::from_str::<serde_json::Value>(&hello) else {
        return;
    };
    if hello.get("method").and_then(serde_json::Value::as_str) != Some("hello")
        || hello.get("token").and_then(serde_json::Value::as_str) != Some(shared.token.as_str())
    {
        return;
    }
    let kind = match hello.get("kind").and_then(serde_json::Value::as_str) {
        Some("vscode") => SessionKind::Vscode,
        _ => SessionKind::TypeScript,
    };

    if kind == SessionKind::TypeScript {
        if let Some(snapshot) = shared.snapshot.lock().expect("snapshot lock").clone() {
            if write_line(&writer, &snapshot).is_err() {
                return;
            }
        }
    }

    let pending: Arc<Mutex<HashMap<i64, crossbeam_channel::Sender<serde_json::Value>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let alive = Arc::new(AtomicBool::new(true));
    shared
        .sessions
        .lock()
        .expect("sessions lock")
        .push(Session {
            kind,
            writer: Arc::clone(&writer),
            pending: Arc::clone(&pending),
            alive: Arc::clone(&alive),
        });

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(id) = message.get("id").and_then(serde_json::Value::as_i64) {
            let result = message
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            if let Some(sender) = pending.lock().expect("pending lock").remove(&id) {
                let _ = sender.send(result);
            }
            continue;
        }
        if message.get("method").and_then(serde_json::Value::as_str) == Some("renameLanded") {
            let symbol = message
                .get("symbol")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let new_name = message
                .get("newName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let files = message
                .get("files")
                .and_then(serde_json::Value::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(PathBuf::from)
                        .collect()
                })
                .unwrap_or_default();
            if !symbol.is_empty() && !new_name.is_empty() {
                let _ = events.send(PluginEvent::RenameLanded {
                    symbol,
                    new_name,
                    files,
                });
            }
        }
    }
    alive.store(false, Ordering::Relaxed);
}

fn write_line(writer: &Arc<Mutex<TcpStream>>, line: &str) -> std::io::Result<()> {
    let mut stream = writer.lock().expect("writer lock");
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// A per-session secret: plugins must echo it from the discovery file, so
/// only processes that can read the workspace can connect.
fn fresh_token() -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    std::process::id().hash(&mut hasher);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut hasher);
    std::time::Instant::now().hash(&mut hasher);
    format!("{:016x}{:016x}", hasher.finish(), {
        let mut second = std::hash::DefaultHasher::new();
        hasher.finish().hash(&mut second);
        second.finish()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;
    use std::time::Instant;

    #[test]
    fn vscode_sessions_do_not_receive_typescript_queries() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let hub = PluginHub::start(dir.path()).expect("hub");
        let mut ts = connect_session(dir.path(), None);
        let _vscode = connect_session(dir.path(), Some("vscode"));
        wait_for_sessions(&hub, 2);

        let mut reader = BufReader::new(ts.try_clone().expect("clone ts"));
        let answer = std::thread::spawn(move || {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read query");
            let query: serde_json::Value = serde_json::from_str(&line).expect("query json");
            let id = query
                .get("id")
                .and_then(serde_json::Value::as_i64)
                .expect("id");
            let response = serde_json::json!({
                "id": id,
                "result": [{ "file": "app.ts", "start": 3, "length": 4 }],
            });
            writeln!(ts, "{response}").expect("write response");
            ts.flush().expect("flush response");
        });

        let locations = hub
            .query_rename_locations(Path::new("generated.ts"), 1, Duration::from_secs(2))
            .expect("locations");
        answer.join().expect("answer thread");

        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].file, "app.ts");
        assert_eq!(locations[0].start, 3);
        assert_eq!(locations[0].length, 4);
    }

    fn connect_session(root: &Path, kind: Option<&str>) -> TcpStream {
        let discovery =
            std::fs::read_to_string(root.join("target/typeweld/ls.json")).expect("discovery file");
        let discovery: serde_json::Value = serde_json::from_str(&discovery).expect("discovery");
        let port = discovery
            .get("port")
            .and_then(serde_json::Value::as_u64)
            .expect("port") as u16;
        let token = discovery
            .get("token")
            .and_then(serde_json::Value::as_str)
            .expect("token");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let hello = match kind {
            Some(kind) => serde_json::json!({ "method": "hello", "token": token, "kind": kind }),
            None => serde_json::json!({ "method": "hello", "token": token }),
        };
        writeln!(stream, "{hello}").expect("write hello");
        stream.flush().expect("flush hello");
        stream
    }

    fn wait_for_sessions(hub: &PluginHub, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if hub.shared.sessions.lock().expect("sessions").len() >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("sessions did not connect");
    }
}

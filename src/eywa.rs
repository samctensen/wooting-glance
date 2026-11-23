#![allow(dead_code)] // Until `glance watch` uses this module.
//! Follows Eywa's client rows over its local API, decoded with `eywa-api`.

use anyhow::{Context, Result, anyhow, ensure};
use eywa_api::{Kind, Message, Request, Row, State};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsStr,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    sync::mpsc,
    thread,
    time::Duration,
};

/// Each Eywa client's state by name, such as `"claude"`.
pub type Clients = BTreeMap<String, State>;

/// Runs Eywa as a background job with `eywa start`.
pub fn start() -> Result<()> {
    run_start(OsStr::new("eywa"))
}

fn run_start(program: &OsStr) -> Result<()> {
    let command = format!("`{} start`", program.to_string_lossy());
    let output = Command::new(program)
        .arg("start")
        .output()
        .with_context(|| format!("cannot run {command}"))?;
    ensure!(
        output.status.success(),
        "{command} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Follows Eywa on a background thread. The receiver gets the clients
/// whenever they change, `None` once each time Eywa becomes unreachable, and
/// an error if Eywa rejects the subscription. If Eywa isn't answering when
/// Glance launches, `start` runs once; an Eywa that stops later stays
/// stopped, since launchd restarts crashes and `eywa stop` should hold.
pub fn subscribe(
    path: PathBuf,
    retry: Duration,
    start: impl Fn() -> Result<()> + Send + 'static,
) -> mpsc::Receiver<Result<Option<Clients>>> {
    let (tx, rx) = mpsc::channel();
    let subscriber = Subscriber {
        path,
        retry,
        tx,
        sent: None,
        outage: true,
        launching: true,
    };
    thread::Builder::new()
        .name("eywa".into())
        .spawn(move || subscriber.run(start))
        .expect("cannot spawn the Eywa thread");
    rx
}

struct Subscriber {
    path: PathBuf,
    retry: Duration,
    tx: mpsc::Sender<Result<Option<Clients>>>,
    sent: Option<Clients>,
    outage: bool,
    launching: bool,
}

enum End {
    Lost,
    Rejected(String),
    Gone,
}

impl Subscriber {
    fn run(mut self, start: impl Fn() -> Result<()>) {
        loop {
            let launching = std::mem::replace(&mut self.launching, false);
            let end = match UnixStream::connect(&self.path) {
                Ok(stream) => self.follow(stream),
                Err(_) if launching => {
                    eprintln!(
                        "Glance: Eywa isn't answering at {}; running eywa start.",
                        self.path.display()
                    );
                    match start() {
                        Ok(()) => continue,
                        Err(e) => eprintln!("Glance: couldn't start Eywa: {e:#}. Waiting for it."),
                    }
                    End::Lost
                }
                Err(_) => End::Lost,
            };
            match end {
                End::Lost => {}
                End::Rejected(reason) => {
                    let _ = self
                        .tx
                        .send(Err(anyhow!("Eywa rejected the subscription: {reason}")));
                    return;
                }
                End::Gone => return,
            }
            if !self.outage {
                self.outage = true;
                eprintln!("Glance: lost Eywa; reconnecting.");
            }
            if !self.emit(None) {
                return;
            }
            thread::sleep(self.retry);
        }
    }

    fn follow(&mut self, mut stream: UnixStream) -> End {
        let request =
            serde_json::to_string(&Request::subscribe("/", 1)).expect("a request encodes");
        if writeln!(stream, "{request}").is_err() {
            return End::Lost;
        }
        let mut rows: HashMap<String, Row> = HashMap::new();
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { return End::Lost };
            let Ok(message) = serde_json::from_str::<Message>(&line) else {
                return End::Lost;
            };
            match message {
                Message::Snapshot { rows: all, .. } => {
                    rows = all.into_iter().map(|row| (row.id.clone(), row)).collect();
                }
                Message::Upsert { row, .. } => {
                    rows.insert(row.id.clone(), row);
                }
                Message::Remove { id, .. } => {
                    rows.remove(&id);
                }
                Message::Error { code, message } => {
                    return End::Rejected(format!("{code}: {message}"));
                }
                Message::Stopping | Message::Other => continue,
            }
            if self.outage {
                self.outage = false;
                eprintln!("Glance: connected to Eywa.");
            }
            if !self.emit(Some(clients(&rows))) {
                return End::Gone;
            }
        }
        End::Lost
    }

    /// Sends `clients` if they changed. False once Glance stops listening.
    fn emit(&mut self, clients: Option<Clients>) -> bool {
        if clients == self.sent {
            return true;
        }
        self.sent = clients.clone();
        self.tx.send(Ok(clients)).is_ok()
    }
}

fn clients(rows: &HashMap<String, Row>) -> Clients {
    rows.values()
        .filter(|row| row.kind == Kind::Client)
        .map(|row| (row.id.trim_start_matches('/').to_string(), row.state))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eywa_api::State::{Idle, NeedsInput, Other, Working};
    use std::{
        ffi::OsStr,
        io::{BufRead, BufReader, Write},
        os::unix::net::{UnixListener, UnixStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering::SeqCst},
            mpsc::{self, Receiver},
        },
    };

    const RETRY: Duration = Duration::from_millis(10);
    const SECOND: Duration = Duration::from_secs(2);

    fn temp_socket() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "glance-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, SeqCst)
        );
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn row(id: &str, kind: &str, state: &str) -> String {
        format!(
            r#"{{"id":"{id}","kind":"{kind}","state":"{state}","since":"2026-11-23T20:41:07.000Z","attrs":{{}}}}"#
        )
    }

    fn snapshot(rows: &[String]) -> String {
        format!(
            r#"{{"v":1,"type":"snapshot","rev":1,"rows":[{}]}}"#,
            rows.join(",")
        )
    }

    fn client(name: &str, state: &str) -> String {
        snapshot(&[
            row("/", "machine", state),
            row(&format!("/{name}"), "client", state),
        ])
    }

    fn accept(listener: &UnixListener, lines: &[String]) -> UnixStream {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&request).unwrap(),
            serde_json::to_value(eywa_api::Request::subscribe("/", 1)).unwrap()
        );
        for line in lines {
            writeln!(stream, "{line}").unwrap();
        }
        stream
    }

    fn next(rx: &Receiver<Result<Option<Clients>>>) -> Option<Clients> {
        rx.recv_timeout(SECOND).unwrap().unwrap()
    }

    fn only(name: &str, state: eywa_api::State) -> Option<Clients> {
        Some(Clients::from([(name.to_string(), state)]))
    }

    fn no_start() -> Result<()> {
        anyhow::bail!("not installed")
    }

    #[test]
    fn subscription_sends_the_request_and_follows_changes() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let rx = subscribe(path.clone(), RETRY, no_start);
        let mut stream = accept(&listener, &[client("claude", "working")]);
        assert_eq!(next(&rx), only("claude", Working));
        let upsert = row("/codex", "client", "needs_input");
        writeln!(
            stream,
            r#"{{"v":1,"type":"upsert","rev":2,"row":{upsert}}}"#
        )
        .unwrap();
        assert_eq!(
            next(&rx),
            Some(Clients::from([
                ("claude".to_string(), Working),
                ("codex".to_string(), NeedsInput),
            ]))
        );
        writeln!(
            stream,
            r#"{{"v":1,"type":"remove","rev":3,"id":"/claude"}}"#
        )
        .unwrap();
        assert_eq!(next(&rx), only("codex", NeedsInput));
    }

    #[test]
    fn lost_connection_goes_dark_and_resubscribes() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let rx = subscribe(path.clone(), RETRY, no_start);
        drop(accept(&listener, &[client("claude", "working")]));
        assert_eq!(next(&rx), only("claude", Working));
        assert_eq!(next(&rx), None);
        let _stream = accept(&listener, &[client("codex", "working")]);
        assert_eq!(next(&rx), only("codex", Working));
    }

    #[test]
    fn eywa_starting_after_glance_is_picked_up() {
        let path = temp_socket();
        let rx = subscribe(path.clone(), RETRY, no_start);
        std::thread::sleep(Duration::from_millis(50));
        let listener = UnixListener::bind(&path).unwrap();
        let _stream = accept(&listener, &[client("muse", "idle")]);
        assert_eq!(next(&rx), only("muse", Idle));
    }

    #[test]
    fn malformed_line_resubscribes() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let rx = subscribe(path.clone(), RETRY, no_start);
        let _first = accept(&listener, &[client("claude", "working"), "garbage".into()]);
        assert_eq!(next(&rx), only("claude", Working));
        assert_eq!(next(&rx), None);
        let _second = accept(&listener, &[client("grok", "needs_input")]);
        assert_eq!(next(&rx), only("grok", NeedsInput));
    }

    #[test]
    fn rejection_ends_the_subscription() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let rx = subscribe(path.clone(), RETRY, no_start);
        let error = r#"{"v":1,"type":"error","code":"bad_request","message":"unknown view"}"#;
        let _stream = accept(&listener, &[error.into()]);
        let error = rx.recv_timeout(SECOND).unwrap().unwrap_err();
        assert!(
            error.to_string().contains("bad_request: unknown view"),
            "{error}"
        );
    }

    #[test]
    fn unknown_messages_and_kinds_are_ignored() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let rx = subscribe(path.clone(), RETRY, no_start);
        let mut stream = accept(&listener, &[client("claude", "working")]);
        assert_eq!(next(&rx), only("claude", Working));
        writeln!(stream, r#"{{"v":1,"type":"heartbeat","rev":2}}"#).unwrap();
        let robot = row("/robot", "robot", "working");
        writeln!(stream, r#"{{"v":1,"type":"upsert","rev":3,"row":{robot}}}"#).unwrap();
        let sleepy = row("/claude", "client", "sleeping");
        writeln!(
            stream,
            r#"{{"v":1,"type":"upsert","rev":4,"row":{sleepy}}}"#
        )
        .unwrap();
        assert_eq!(next(&rx), only("claude", Other));
    }

    #[test]
    fn eywa_is_started_at_launch_but_a_later_stop_is_respected() {
        let path = temp_socket();
        let (tx, started) = mpsc::channel();
        let socket = path.clone();
        let rx = subscribe(path.clone(), RETRY, move || {
            let _ = std::fs::remove_file(&socket);
            tx.send(UnixListener::bind(&socket)?).unwrap();
            Ok(())
        });
        let listener = started.recv_timeout(SECOND).unwrap();
        let stream = accept(&listener, &[client("claude", "working")]);
        assert_eq!(next(&rx), only("claude", Working));
        // As if someone ran `eywa stop`.
        drop((listener, stream));
        assert_eq!(next(&rx), None);
        assert!(
            started.recv_timeout(Duration::from_millis(200)).is_err(),
            "Glance restarted an Eywa that was stopped"
        );
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let _stream = accept(&listener, &[client("codex", "working")]);
        assert_eq!(next(&rx), only("codex", Working));
    }

    #[test]
    fn a_running_eywa_is_not_restarted_after_it_stops() {
        let path = temp_socket();
        let listener = UnixListener::bind(&path).unwrap();
        let starts = Arc::new(AtomicUsize::new(0));
        let count = starts.clone();
        let rx = subscribe(path, RETRY, move || {
            count.fetch_add(1, SeqCst);
            Ok(())
        });
        let stream = accept(&listener, &[client("claude", "working")]);
        assert_eq!(next(&rx), only("claude", Working));
        drop((listener, stream));
        assert_eq!(next(&rx), None);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(starts.load(SeqCst), 0);
    }

    #[test]
    fn start_command_reports_failure() {
        assert!(run_start(OsStr::new("/usr/bin/true")).is_ok());
        let failed = run_start(OsStr::new("/usr/bin/false")).unwrap_err();
        assert!(failed.to_string().contains("start"), "{failed}");
        assert!(run_start(OsStr::new("/nonexistent/eywa")).is_err());
    }
}

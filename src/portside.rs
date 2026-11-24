//! A client for Portside's local game API (Portside 0.16.0 and later).
//!
//! Portside (KittyConnect) listens on 127.0.0.1, ports 47800 to 47809, one
//! JSON object per line each way. Through it this game sees the hub you're
//! in, shows a game you host to everyone in that hub who has Ink-cognito
//! open, and joins theirs. A session's traffic is UDP that Portside carries
//! over its VOIP helper (the TailCat copy that does UDP), WireGuard-encrypted;
//! this game only ever sends to 127.0.0.1. See KittyConnect's docs/game-api.md.
//!
//! The connection lives on a thread of its own and comes back by itself when
//! Portside starts later or restarts.

use bevy::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc as std_mpsc},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::mpsc,
    time::timeout,
};

/// The same id on every PC: sessions only reach the same game.
pub const APP_ID: &str = "com.javacrafter.ink-cognito";
pub const APP_NAME: &str = "Ink-cognito";
const PORTS: std::ops::RangeInclusive<u16> = 47800..=47809;
const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Looking for Portside.
    Looking,
    /// Portside isn't running (or its game API is off).
    NotRunning,
    /// Portside is asking the user whether to allow this game.
    Asking,
    Ready,
    /// The user said "Don't allow". We don't ask again until they press retry.
    Denied(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hub {
    pub id: String,
    pub name: String,
    /// False when the hub's host runs a Portside older than 0.16.0.
    pub games: bool,
    pub members: usize,
}

/// A game of Ink-cognito someone in the hub is hosting.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// "<host's member id>:<sid>", what `join` takes.
    pub id: String,
    pub title: String,
    pub host_name: String,
    pub mine: bool,
    pub players: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Hosted {
        sid: String,
    },
    HostFailed(String),
    Joined {
        session: String,
        port: u16,
        title: String,
    },
    JoinFailed {
        session: String,
        error: String,
    },
    /// A session's tunnel stopped: ours (`sid`) or one we joined (`id`).
    SessionEnded {
        sid: Option<String>,
        id: Option<String>,
        error: String,
    },
    /// The connection to Portside closed: everything hosted or joined through it ended.
    Lost,
}

#[derive(Debug)]
enum Cmd {
    Host {
        port: u16,
        title: String,
        info: Value,
    },
    Update {
        sid: String,
        info: Value,
    },
    Stop {
        sid: String,
    },
    Join {
        session: String,
        title: String,
    },
    Leave {
        session: String,
    },
    Activity(Option<String>),
    Retry,
}

#[derive(Debug, Clone)]
struct Shared {
    status: Status,
    my_name: Option<String>,
    hub: Option<Hub>,
    sessions: Vec<Session>,
    /// Bumped on every change, so the UI knows when to redraw.
    version: u64,
}

/// What the game sees of Portside, updated from the connection's thread.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub status: Status,
    pub my_name: Option<String>,
    pub hub: Option<Hub>,
    pub sessions: Vec<Session>,
    pub version: u64,
}

impl Snapshot {
    /// True when a hosted game can be shown in the hub.
    pub fn can_host(&self) -> bool {
        self.status == Status::Ready && self.hub.as_ref().is_some_and(|h| h.games)
    }
}

#[derive(Resource)]
pub struct Portside {
    shared: Arc<Mutex<Shared>>,
    tx: mpsc::UnboundedSender<Cmd>,
    events: Mutex<std_mpsc::Receiver<Event>>,
}

impl Default for Portside {
    fn default() -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            status: Status::Looking,
            my_name: None,
            hub: None,
            sessions: Vec::new(),
            version: 0,
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = std_mpsc::channel();
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("portside-api".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build Tokio runtime for Portside");
                rt.block_on(run(thread_shared, rx, ev_tx));
            });
        if let Err(err) = spawned {
            warn!("Portside: couldn't start its thread: {err}");
        }
        Self {
            shared,
            tx,
            events: Mutex::new(ev_rx),
        }
    }
}

impl Portside {
    pub fn snapshot(&self) -> Snapshot {
        let s = self
            .shared
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|e| e.into_inner().clone());
        Snapshot {
            status: s.status,
            my_name: s.my_name,
            hub: s.hub,
            sessions: s.sessions,
            version: s.version,
        }
    }

    pub fn poll_events(&self) -> Vec<Event> {
        match self.events.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn host(&self, port: u16, title: String, info: Value) {
        let _ = self.tx.send(Cmd::Host { port, title, info });
    }
    pub fn update(&self, sid: String, info: Value) {
        let _ = self.tx.send(Cmd::Update { sid, info });
    }
    pub fn stop(&self, sid: String) {
        let _ = self.tx.send(Cmd::Stop { sid });
    }
    pub fn join(&self, session: String, title: String) {
        let _ = self.tx.send(Cmd::Join { session, title });
    }
    pub fn leave(&self, session: String) {
        let _ = self.tx.send(Cmd::Leave { session });
    }
    pub fn set_activity(&self, text: Option<String>) {
        let _ = self.tx.send(Cmd::Activity(text));
    }
    /// Try again now (after "Don't allow", or to skip the wait).
    pub fn retry(&self) {
        let _ = self.tx.send(Cmd::Retry);
    }
}

fn update(shared: &Mutex<Shared>, f: impl FnOnce(&mut Shared)) {
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut s);
    s.version += 1;
}

async fn run(
    shared: Arc<Mutex<Shared>>,
    mut cmds: mpsc::UnboundedReceiver<Cmd>,
    events: std_mpsc::Sender<Event>,
) {
    let mut announced_missing = false;
    loop {
        update(&shared, |s| {
            if s.status != Status::NotRunning {
                s.status = Status::Looking;
            }
        });
        let wait = match connect().await {
            None => {
                if !announced_missing {
                    info!("Portside isn't running; hub games are off until it is.");
                    announced_missing = true;
                }
                update(&shared, |s| s.status = Status::NotRunning);
                Wait::Retry(Duration::from_secs(4))
            }
            Some((reader, writer)) => {
                announced_missing = false;
                let end = session(reader, writer, &shared, &mut cmds, &events).await;
                update(&shared, |s| {
                    s.hub = None;
                    s.sessions.clear();
                    s.my_name = None;
                });
                let _ = events.send(Event::Lost);
                match end {
                    End::Denied(msg) => {
                        update(&shared, |s| s.status = Status::Denied(msg));
                        Wait::UntilRetry
                    }
                    End::Closed => {
                        update(&shared, |s| s.status = Status::NotRunning);
                        Wait::Retry(Duration::from_secs(2))
                    }
                    End::Quit => return,
                }
            }
        };
        // Wait, answering anything asked meanwhile with "not connected".
        let deadline = match wait {
            Wait::Retry(d) => Some(tokio::time::Instant::now() + d),
            Wait::UntilRetry => None,
        };
        loop {
            let cmd = match deadline {
                Some(d) => match tokio::time::timeout_at(d, cmds.recv()).await {
                    Ok(cmd) => cmd,
                    Err(_) => break,
                },
                None => cmds.recv().await,
            };
            match cmd {
                None => return,
                Some(Cmd::Retry) => break,
                Some(cmd) => fail_offline(cmd, &events),
            }
        }
    }
}

enum Wait {
    Retry(Duration),
    UntilRetry,
}

enum End {
    Closed,
    Denied(String),
    Quit,
}

fn fail_offline(cmd: Cmd, events: &std_mpsc::Sender<Event>) {
    let why = "Portside isn't connected.".to_string();
    match cmd {
        Cmd::Host { .. } => {
            let _ = events.send(Event::HostFailed(why));
        }
        Cmd::Join { session, .. } => {
            let _ = events.send(Event::JoinFailed {
                session,
                error: why,
            });
        }
        _ => {}
    }
}

/// Finds Portside: the port that greets with `{"op":"portside"}`.
async fn connect() -> Option<(BufReader<OwnedReadHalf>, OwnedWriteHalf)> {
    for port in PORTS {
        let Ok(Ok(stream)) = timeout(
            Duration::from_millis(400),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let (r, w) = stream.into_split();
        let mut reader = BufReader::new(r);
        let Ok(Ok(Some(hi))) = timeout(Duration::from_secs(2), read_line(&mut reader)).await else {
            continue;
        };
        if hi["op"] == "portside" {
            info!(
                "Found Portside {} on port {port}",
                hi["version"].as_str().unwrap_or("?")
            );
            return Some((reader, w));
        }
    }
    None
}

async fn read_line(reader: &mut BufReader<OwnedReadHalf>) -> std::io::Result<Option<Value>> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = (&mut *reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut buf)
            .await?;
        if n == 0 {
            return Ok(None);
        }
        if buf.len() > MAX_LINE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "line too long",
            ));
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        return serde_json::from_str(line)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
    }
}

async fn send(writer: &mut OwnedWriteHalf, v: &Value) -> std::io::Result<()> {
    let mut line = v.to_string();
    line.push('\n');
    writer.write_all(line.as_bytes()).await
}

enum Pending {
    Me,
    Host,
    Join { session: String, title: String },
    Other(&'static str),
}

async fn session(
    mut reader: BufReader<OwnedReadHalf>,
    mut writer: OwnedWriteHalf,
    shared: &Mutex<Shared>,
    cmds: &mut mpsc::UnboundedReceiver<Cmd>,
    events: &std_mpsc::Sender<Event>,
) -> End {
    let mut hello = json!({"op": "hello", "v": 1, "app": {"id": APP_ID, "name": APP_NAME}});
    if let Some(token) = load_token() {
        hello["token"] = json!(token);
    }
    if send(&mut writer, &hello).await.is_err() {
        return End::Closed;
    }

    // Hello → (asking → authorized) → ready.
    loop {
        let msg = tokio::select! {
            line = read_line(&mut reader) => match line {
                Ok(Some(msg)) => msg,
                _ => return End::Closed,
            },
            cmd = cmds.recv() => match cmd {
                None => return End::Quit,
                Some(Cmd::Retry) => continue,
                Some(cmd) => { fail_offline(cmd, events); continue }
            },
        };
        match msg["op"].as_str() {
            Some("asking") => {
                info!("Portside is asking whether to allow Ink-cognito.");
                update(shared, |s| s.status = Status::Asking);
            }
            Some("authorized") => {
                if let Some(token) = msg["token"].as_str() {
                    save_token(token);
                }
            }
            Some("ready") => break,
            Some("denied") => {
                let why = msg["error"].as_str().unwrap_or("Not allowed in Portside.");
                // A key that stopped working (say, the game moved) is no use: ask afresh next time.
                if load_token().is_some() {
                    clear_token();
                    return End::Closed;
                }
                return End::Denied(why.to_string());
            }
            _ => {}
        }
    }
    update(shared, |s| s.status = Status::Ready);
    info!("Connected to Portside's game API.");

    let mut next_id: u64 = 1;
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut request = |cmd: Value, what: Pending, pending: &mut HashMap<u64, Pending>| {
        let id = next_id;
        next_id += 1;
        pending.insert(id, what);
        let mut v = cmd;
        v["id"] = json!(id);
        v
    };

    let first = request(json!({"cmd": "getMe"}), Pending::Me, &mut pending);
    if send(&mut writer, &first).await.is_err() {
        return End::Closed;
    }

    loop {
        tokio::select! {
            line = read_line(&mut reader) => {
                let msg = match line {
                    Ok(Some(msg)) => msg,
                    Ok(None) => return End::Closed,
                    Err(err) => {
                        warn!("Portside: {err}");
                        return End::Closed;
                    }
                };
                if let Some(evt) = msg["evt"].as_str() {
                    on_event(evt, &msg["data"], shared, events);
                    continue;
                }
                let Some(id) = msg["id"].as_u64() else { continue };
                let Some(what) = pending.remove(&id) else { continue };
                let ok = msg["ok"].as_bool() == Some(true);
                let error = msg["error"].as_str().unwrap_or("Portside said no.").to_string();
                let data = &msg["data"];
                match what {
                    Pending::Me if ok => {
                        let name = data["name"].as_str().map(str::to_string);
                        update(shared, |s| s.my_name = name);
                    }
                    Pending::Me => {}
                    Pending::Host if ok => {
                        let sid = data["sid"].as_str().unwrap_or_default().to_string();
                        let _ = events.send(Event::Hosted { sid });
                    }
                    Pending::Host => {
                        let _ = events.send(Event::HostFailed(error));
                    }
                    Pending::Join { session, title } => {
                        let port = data["port"].as_u64().and_then(|p| u16::try_from(p).ok());
                        let ev = match port {
                            Some(port) if ok => Event::Joined { session, port, title },
                            _ => Event::JoinFailed { session, error },
                        };
                        let _ = events.send(ev);
                    }
                    Pending::Other(cmd) => {
                        if !ok {
                            warn!("Portside {cmd}: {error}");
                        }
                    }
                }
            }
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { return End::Quit };
                let req = match cmd {
                    Cmd::Host { port, title, info } => request(
                        json!({"cmd": "hostSession", "port": port, "title": title, "info": info}),
                        Pending::Host, &mut pending),
                    Cmd::Update { sid, info } => request(
                        json!({"cmd": "updateSession", "sid": sid, "info": info}),
                        Pending::Other("updateSession"), &mut pending),
                    Cmd::Stop { sid } => request(
                        json!({"cmd": "stopSession", "sid": sid}),
                        Pending::Other("stopSession"), &mut pending),
                    Cmd::Join { session, title } => request(
                        json!({"cmd": "joinSession", "session": session}),
                        Pending::Join { session, title }, &mut pending),
                    Cmd::Leave { session } => request(
                        json!({"cmd": "leaveSession", "session": session}),
                        Pending::Other("leaveSession"), &mut pending),
                    Cmd::Activity(Some(text)) => request(
                        json!({"cmd": "setActivity", "text": text}),
                        Pending::Other("setActivity"), &mut pending),
                    Cmd::Activity(None) => request(
                        json!({"cmd": "clearActivity"}),
                        Pending::Other("clearActivity"), &mut pending),
                    Cmd::Retry => continue,
                };
                if send(&mut writer, &req).await.is_err() {
                    return End::Closed;
                }
            }
        }
    }
}

fn on_event(evt: &str, data: &Value, shared: &Mutex<Shared>, events: &std_mpsc::Sender<Event>) {
    match evt {
        "hub" => {
            let hub = parse_hub(data);
            update(shared, |s| {
                if hub.is_none() {
                    s.sessions.clear();
                }
                s.hub = hub;
            });
        }
        "sessions" => {
            let sessions = parse_sessions(data);
            update(shared, |s| s.sessions = sessions);
        }
        "sessionEnded" => {
            let _ = events.send(Event::SessionEnded {
                sid: data["sid"].as_str().map(str::to_string),
                id: data["id"].as_str().map(str::to_string),
                error: data["error"]
                    .as_str()
                    .unwrap_or("The session ended.")
                    .to_string(),
            });
        }
        _ => {}
    }
}

pub(crate) fn parse_hub(v: &Value) -> Option<Hub> {
    if !v.is_object() {
        return None;
    }
    Some(Hub {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        name: v["name"].as_str().unwrap_or("your hub").to_string(),
        games: v["games"].as_bool().unwrap_or(false),
        members: v["members"].as_array().map_or(0, Vec::len),
    })
}

pub(crate) fn parse_sessions(v: &Value) -> Vec<Session> {
    v.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|s| {
                    Some(Session {
                        id: s["id"].as_str()?.to_string(),
                        title: s["title"].as_str().unwrap_or("A game").to_string(),
                        host_name: s["host"]["name"].as_str().unwrap_or("Someone").to_string(),
                        mine: s["mine"].as_bool().unwrap_or(false),
                        players: s["info"]["players"].as_u64(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where the key Portside gave us is kept (it only works from this program's file).
fn token_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("INK_COGNITO_PORTSIDE_TOKEN") {
        return Some(PathBuf::from(p));
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("ink-cognito").join("portside-token.json"))
}

fn load_token() -> Option<String> {
    let bytes = std::fs::read(token_path()?).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v["token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

fn save_token(token: &str) {
    let Some(path) = token_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(err) = std::fs::write(&path, json!({"token": token}).to_string()) {
        warn!(
            "Portside: couldn't save the key at {}: {err}",
            path.display()
        );
    }
}

fn clear_token() {
    if let Some(path) = token_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hub_and_sessions_like_the_docs() {
        let hub = json!({"id":"h1","name":"Kit's hub","saved":false,"host":true,"me":1,"games":true,
            "members":[{"id":1,"name":"Kit"},{"id":2,"name":"Jo"}]});
        let hub = parse_hub(&hub).unwrap();
        assert_eq!(hub.name, "Kit's hub");
        assert!(hub.games);
        assert_eq!(hub.members, 2);
        assert!(parse_hub(&Value::Null).is_none());

        let sessions = json!([{"id":"2:1a2b3c4d","sid":"1a2b3c4d","title":"Jo's plate",
            "info":{"players":3},"host":{"id":2,"name":"Jo"},"mine":false}]);
        let sessions = parse_sessions(&sessions);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "2:1a2b3c4d");
        assert_eq!(sessions[0].host_name, "Jo");
        assert_eq!(sessions[0].players, Some(3));
        assert!(!sessions[0].mine);
    }

    /// Talks to a stand-in Portside that answers like docs/game-api.md says.
    #[test]
    fn talks_to_portside_from_hello_to_join() {
        use std::io::{BufRead, BufReader as StdReader, Write};
        use std::net::TcpListener;

        let dir = std::env::temp_dir().join(format!("ink-portside-test-{}", std::process::id()));
        let token_file = dir.join("token.json");
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("INK_COGNITO_PORTSIDE_TOKEN", &token_file) };

        let listener = TcpListener::bind(("127.0.0.1", 47800))
            .expect("port 47800 is taken (is Portside running here?)");
        let fake = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            let mut lines = StdReader::new(stream).lines();
            let mut say = |v: Value| {
                out.write_all(format!("{v}\n").as_bytes()).unwrap();
            };
            say(json!({"op":"portside","v":1,"version":"0.16.0"}));
            let hello: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
            assert_eq!(hello["op"], "hello");
            assert_eq!(hello["app"]["id"], APP_ID);
            assert!(hello.get("token").is_none(), "no key the first time");
            say(json!({"op":"asking"}));
            say(json!({"op":"authorized","token":"k1"}));
            say(json!({"op":"ready","v":1,"app":APP_ID}));
            say(
                json!({"evt":"hub","data":{"id":"h","name":"Jo's hub","games":true,"members":[{"id":1},{"id":2}]}}),
            );
            say(
                json!({"evt":"sessions","data":[{"id":"2:feed","sid":"feed","title":"Jo's plate",
                "info":{"players":2},"host":{"id":2,"name":"Jo"},"mine":false}]}),
            );
            let mut seen = Vec::new();
            for line in lines {
                let req: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let id = req["id"].clone();
                let cmd = req["cmd"].as_str().unwrap().to_string();
                let data = match cmd.as_str() {
                    "getMe" => json!({"name":"Kit","inHub":true,"inVoice":false}),
                    "hostSession" => {
                        assert_eq!(req["port"], 4242);
                        assert_eq!(req["title"], "Kit's plate");
                        json!({"sid":"abcd1234","port":4242})
                    }
                    "joinSession" => {
                        assert_eq!(req["session"], "2:feed");
                        json!({"id":"2:feed","port":5555})
                    }
                    _ => json!({}),
                };
                say(json!({"id":id,"ok":true,"data":data}));
                seen.push(cmd.clone());
                if cmd == "joinSession" {
                    break;
                }
            }
            seen
        });

        let portside = Portside::default();
        let mut ready = false;
        for _ in 0..200 {
            let s = portside.snapshot();
            if s.status == Status::Ready && s.sessions.len() == 1 && s.my_name.is_some() {
                assert!(s.can_host());
                assert_eq!(s.hub.unwrap().name, "Jo's hub");
                assert_eq!(s.my_name.as_deref(), Some("Kit"));
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ready, "never got ready with the hub and sessions");
        assert_eq!(load_token().as_deref(), Some("k1"), "the key is saved");

        let seen_events = std::cell::RefCell::new(Vec::new());
        let wait_for = |want: &dyn Fn(&Event) -> bool| {
            for _ in 0..200 {
                let mut log = seen_events.borrow_mut();
                log.extend(portside.poll_events());
                if let Some(i) = log.iter().position(want) {
                    log.remove(i);
                    return true;
                }
                drop(log);
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        portside.host(4242, "Kit's plate".into(), json!({"players":1}));
        assert!(wait_for(
            &|e| matches!(e, Event::Hosted { sid } if sid == "abcd1234")
        ));
        portside.join("2:feed".into(), "Jo's plate".into());
        assert!(wait_for(&|e| matches!(e, Event::Joined { port: 5555, .. })));

        let seen = fake.join().unwrap();
        assert_eq!(seen.first().map(String::as_str), Some("getMe"));
        // The fake hung up: the client notices and says so.
        assert!(wait_for(&|e| matches!(e, Event::Lost)));
        let _ = std::fs::remove_dir_all(dir);
    }
}

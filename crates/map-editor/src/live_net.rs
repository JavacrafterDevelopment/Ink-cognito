//! The connections behind live editing, the same two ways the game connects:
//!
//! - **Portside**: the host listens on a TCP port on 127.0.0.1 and shows it
//!   as a session in the Portside hub; a friend's Portside hands them a local
//!   port that leads to it. (The game uses UDP for its sessions; the editor
//!   uses TCP, which Portside carries too, because edits mustn't get lost.)
//! - **TailCat**: the host also gets a `tc...` address anyone can paste.
//!
//! Either way it's one stream per guest carrying length-prefixed JSON frames.
//! This side only moves messages; `live.rs` decides what they mean.

use crate::live_sync::Op;
use bevy::prelude::*;
use ink_map::MapFile;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    future::Future,
    net::Ipv4Addr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tailcat::{Client, Server, ServerConfig, TcpHandler};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    sync::{Notify, mpsc},
};

/// The TailCat port map editing runs on (the game's is 7777).
pub const EDIT_PORT: u16 = 7790;
/// Bumped when the messages change: older editors are told to update.
pub const VERSION: u32 = 1;
/// A whole map fits easily (the biggest built-in one is about 11 KB).
const MAX_FRAME: usize = 8 * 1024 * 1024;
const PING_EVERY: Duration = Duration::from_secs(3);
/// Nothing heard for this long: the other end is gone.
const SILENCE: Duration = Duration::from_secs(30);
/// How long joining keeps trying while Portside's tunnel comes up.
const JOIN_FOR: Duration = Duration::from_secs(30);

/// Someone in the session. The host is id 0.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Person {
    pub id: u32,
    pub name: String,
}

/// Where someone's camera is and what they've selected, so others can see them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Look {
    pub id: u32,
    pub eye: [f32; 3],
    pub dir: [f32; 3],
    /// The selected piece or spawn's spot, if any.
    pub sel: Option<[f32; 3]>,
}

/// A guest to the host.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ToHost {
    Hello { name: String, version: u32 },
    /// My changes; `seq` counts up so the host can say which it has applied.
    Ops { seq: u64, ops: Vec<Op> },
    /// I'm out of step: send the whole map.
    Resync,
    Look(Look),
    Ping,
    Bye,
}

/// The host to a guest.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ToGuest {
    Welcome { you: u32, file_name: String, map: Box<MapFile>, hash: u64 },
    /// Changes, the host's map hash after them, and the last of your `seq`s
    /// the host has applied.
    Ops { ops: Vec<Op>, hash: u64, ack: u64 },
    People(Vec<Person>),
    Looks(Vec<Look>),
    Ping,
    Bye { why: String },
}

/// What the connections tell the editor.
#[derive(Debug)]
pub enum NetIn {
    /// Hosting: Portside sessions can reach us on this 127.0.0.1 TCP port.
    Listening { port: u16 },
    TailcatReady(String),
    TailcatFailed(String),
    GuestJoined { id: u32, name: String, version: u32 },
    GuestLeft { id: u32 },
    FromGuest { id: u32, msg: ToHost },
    /// Joining: the host answered.
    FromHost(ToGuest),
    /// The session is over (or never started), and why.
    Ended(String),
}

/// What the editor tells the connections.
#[derive(Debug)]
pub enum NetOut {
    To(u32, ToGuest),
    /// To every guest but `except`.
    All { except: Option<u32>, msg: ToGuest },
    /// Joining: to the host.
    Host(ToHost),
}

/// Where to join.
#[derive(Debug, Clone)]
pub enum Target {
    Tailcat(String),
    /// Portside's local port for the session.
    Local(u16),
}

struct Link {
    tx: mpsc::UnboundedSender<NetOut>,
    rx: Mutex<mpsc::UnboundedReceiver<NetIn>>,
    shutdown: Arc<Notify>,
}

/// The connections, run on a Tokio runtime of their own.
pub struct LiveNet {
    runtime: Runtime,
    link: Option<Link>,
}

impl Default for LiveNet {
    fn default() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("live-edit-net")
            .build()
            .expect("failed to build Tokio runtime for live editing");
        Self { runtime, link: None }
    }
}

impl LiveNet {
    pub fn active(&self) -> bool {
        self.link.is_some()
    }

    fn start<F, Fut>(&mut self, run: F)
    where
        F: FnOnce(mpsc::UnboundedReceiver<NetOut>, mpsc::UnboundedSender<NetIn>, Arc<Notify>) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.stop();
        let (tx, out_rx) = mpsc::unbounded_channel();
        let (in_tx, rx) = mpsc::unbounded_channel();
        let shutdown = Arc::new(Notify::new());
        self.runtime.spawn(run(out_rx, in_tx, shutdown.clone()));
        self.link = Some(Link { tx, rx: Mutex::new(rx), shutdown });
    }

    pub fn host(&mut self) {
        self.start(run_host);
    }

    pub fn join(&mut self, target: Target, name: String) {
        self.start(move |out, inn, shutdown| run_guest(target, name, out, inn, shutdown));
    }

    pub fn send(&self, msg: NetOut) {
        if let Some(link) = &self.link {
            let _ = link.tx.send(msg);
        }
    }

    pub fn poll(&self) -> Vec<NetIn> {
        match &self.link {
            Some(link) => match link.rx.lock() {
                Ok(mut rx) => std::iter::from_fn(|| rx.try_recv().ok()).collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    /// Ends the session (guests are told goodbye).
    pub fn stop(&mut self) {
        if let Some(link) = self.link.take() {
            link.shutdown.notify_one();
        }
    }
}

impl Drop for LiveNet {
    fn drop(&mut self) {
        self.stop();
        // Give the goodbyes a moment to go out.
        std::thread::sleep(Duration::from_millis(250));
    }
}

// --- Frames -------------------------------------------------------------------

async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, msg: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too big"));
    }
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await
}

/// `Ok(None)`: the other end closed.
async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> std::io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("bad frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map(Some).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Reads frames on a task of its own (reading isn't safe to cancel halfway)
/// into a channel; it closes when the stream does.
fn spawn_reader<T, R>(mut r: R) -> (mpsc::UnboundedReceiver<T>, tokio::task::JoinHandle<()>)
where
    T: DeserializeOwned + Send + 'static,
    R: AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Ok(Some(msg)) = read_frame::<_, T>(&mut r).await {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    (rx, task)
}

// --- Hosting --------------------------------------------------------------------

type Guests = Arc<Mutex<HashMap<u32, mpsc::UnboundedSender<ToGuest>>>>;

async fn run_host(mut out: mpsc::UnboundedReceiver<NetOut>, inn: mpsc::UnboundedSender<NetIn>, shutdown: Arc<Notify>) {
    let guests: Guests = Arc::new(Mutex::new(HashMap::new()));
    let next_id = Arc::new(AtomicU32::new(1));

    // The local port first: it's instant, and it's what Portside sessions reach.
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(l) => l,
        Err(e) => {
            let _ = inn.send(NetIn::Ended(format!("Couldn't start hosting: {e}")));
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    info!("Live editing: listening for Portside guests on 127.0.0.1:{port} (TCP)");
    let _ = inn.send(NetIn::Listening { port });
    let accept = {
        let (guests, next_id, inn) = (guests.clone(), next_id.clone(), inn.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                };
                let _ = stream.set_nodelay(true);
                tokio::spawn(serve_guest(stream, guests.clone(), next_id.clone(), inn.clone()));
            }
        })
    };

    // TailCat, for a pasted address, the same way the game hosts.
    let tcp_handler: TcpHandler = {
        let (guests, next_id, inn) = (guests.clone(), next_id.clone(), inn.clone());
        Arc::new(move |stream: DuplexStream| {
            let (guests, next_id, inn) = (guests.clone(), next_id.clone(), inn.clone());
            Box::pin(async move {
                serve_guest(stream, guests, next_id, inn).await;
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        })
    };
    let config = ServerConfig {
        on_tcp: Some(Arc::new(move |port| (port == EDIT_PORT).then(|| tcp_handler.clone()))),
        ..Default::default()
    };

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let start = Server::start(config);
    tokio::pin!(start);
    let mut server = None;
    let mut starting = true;
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            msg = out.recv() => match msg {
                Some(msg) => route(&guests, msg),
                None => break,
            },
            _ = ping.tick() => route(&guests, NetOut::All { except: None, msg: ToGuest::Ping }),
            res = &mut start, if starting => {
                starting = false;
                match res {
                    Ok(s) => {
                        let addr = s.tailcat_addr();
                        info!("Live editing: TailCat address ready:\n{addr}");
                        let _ = inn.send(NetIn::TailcatReady(addr));
                        server = Some(s);
                    }
                    Err(e) => {
                        let _ = inn.send(NetIn::TailcatFailed(format!("No TailCat address: {e}")));
                    }
                }
            }
        }
    }

    // Goodbye to everyone, a moment for it to go out, then stop.
    route(&guests, NetOut::All { except: None, msg: ToGuest::Bye { why: "The host stopped live editing.".into() } });
    tokio::time::sleep(Duration::from_millis(200)).await;
    accept.abort();
    guests.lock().unwrap_or_else(|e| e.into_inner()).clear();
    if let Some(s) = server {
        s.close().await;
    }
}

fn route(guests: &Guests, msg: NetOut) {
    let guests = guests.lock().unwrap_or_else(|e| e.into_inner());
    match msg {
        NetOut::To(id, m) => {
            if let Some(tx) = guests.get(&id) {
                let _ = tx.send(m);
            }
        }
        NetOut::All { except, msg } => {
            for (id, tx) in guests.iter() {
                if Some(*id) != except {
                    let _ = tx.send(msg.clone());
                }
            }
        }
        NetOut::Host(_) => {}
    }
}

/// One guest's connection, Portside's or TailCat's.
async fn serve_guest<S>(stream: S, guests: Guests, next_id: Arc<AtomicU32>, inn: mpsc::UnboundedSender<NetIn>)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (r, mut w) = tokio::io::split(stream);
    let (mut frames, reader) = spawn_reader::<ToHost, _>(r);
    let Ok(Some(ToHost::Hello { name, version })) = tokio::time::timeout(SILENCE, frames.recv()).await else {
        reader.abort();
        return;
    };
    let id = next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<ToGuest>();
    guests.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
    let _ = inn.send(NetIn::GuestJoined { id, name, version });

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let bye = matches!(msg, ToGuest::Bye { .. });
            if write_frame(&mut w, &msg).await.is_err() || bye {
                break;
            }
        }
        let _ = w.shutdown().await;
    });

    loop {
        match tokio::time::timeout(SILENCE, frames.recv()).await {
            Ok(Some(ToHost::Bye)) | Ok(None) | Err(_) => break,
            Ok(Some(ToHost::Ping)) => {}
            Ok(Some(msg)) => {
                let _ = inn.send(NetIn::FromGuest { id, msg });
            }
        }
    }
    reader.abort();
    // Dropping its sender ends the writer once what's queued has gone out.
    let tx = guests.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    let _ = inn.send(NetIn::GuestLeft { id });
}

// --- Joining --------------------------------------------------------------------

async fn run_guest(
    target: Target,
    name: String,
    mut out: mpsc::UnboundedReceiver<NetOut>,
    inn: mpsc::UnboundedSender<NetIn>,
    shutdown: Arc<Notify>,
) {
    let why = match target {
        Target::Tailcat(addr) => {
            let client = tokio::select! {
                _ = shutdown.notified() => return,
                c = Client::connect(&addr, None, None) => c,
            };
            match client {
                Err(e) => format!("Couldn't connect over TailCat: {e}"),
                Ok(client) => {
                    let why = match client.dial_tcp_port(EDIT_PORT).await {
                        Ok(stream) => match hello(stream, &name).await {
                            Ok(conn) => guest_session(conn, &mut out, &inn, &shutdown).await,
                            Err(e) => e,
                        },
                        Err(e) => format!("Reached the host, but not their map editor: {e}"),
                    };
                    client.close().await;
                    why
                }
            }
        }
        Target::Local(port) => {
            // Portside's tunnel can take a few seconds to come up: keep trying.
            let start = tokio::time::Instant::now();
            loop {
                let attempt = async {
                    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.map_err(|e| e.to_string())?;
                    let _ = stream.set_nodelay(true);
                    hello(stream, &name).await
                };
                let res = tokio::select! {
                    _ = shutdown.notified() => return,
                    r = attempt => r,
                };
                match res {
                    Ok(conn) => break guest_session(conn, &mut out, &inn, &shutdown).await,
                    Err(e) if start.elapsed() > JOIN_FOR => break format!("Couldn't reach the host through Portside: {e}"),
                    Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
                }
            }
        }
    };
    let _ = inn.send(NetIn::Ended(why));
}

/// A connection that got the host's first answer.
struct Joined<W> {
    writer: W,
    frames: mpsc::UnboundedReceiver<ToGuest>,
    reader: tokio::task::JoinHandle<()>,
    first: ToGuest,
}

/// Says hello and waits for the host's first message.
async fn hello<S>(stream: S, name: &str) -> Result<Joined<tokio::io::WriteHalf<S>>, String>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (r, mut w) = tokio::io::split(stream);
    let (mut frames, reader) = spawn_reader::<ToGuest, _>(r);
    let hi = ToHost::Hello { name: name.to_string(), version: VERSION };
    if let Err(e) = write_frame(&mut w, &hi).await {
        reader.abort();
        return Err(e.to_string());
    }
    match tokio::time::timeout(Duration::from_secs(10), frames.recv()).await {
        Ok(Some(first)) => Ok(Joined { writer: w, frames, reader, first }),
        Ok(None) => {
            reader.abort();
            Err("the connection closed".into())
        }
        Err(_) => {
            reader.abort();
            Err("no answer".into())
        }
    }
}

/// Carries messages both ways until either end stops. Returns why it ended.
async fn guest_session<W: AsyncWrite + Unpin>(
    mut conn: Joined<W>,
    out: &mut mpsc::UnboundedReceiver<NetOut>,
    inn: &mpsc::UnboundedSender<NetIn>,
    shutdown: &Notify,
) -> String {
    if let ToGuest::Bye { why } = &conn.first {
        conn.reader.abort();
        return why.clone();
    }
    let _ = inn.send(NetIn::FromHost(conn.first.clone()));
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heard = tokio::time::Instant::now();
    let why = loop {
        tokio::select! {
            _ = shutdown.notified() => {
                let _ = write_frame(&mut conn.writer, &ToHost::Bye).await;
                break String::new();
            }
            msg = out.recv() => match msg {
                Some(NetOut::Host(m)) => {
                    if write_frame(&mut conn.writer, &m).await.is_err() {
                        break "Lost the connection to the host.".into();
                    }
                }
                Some(_) => {}
                None => {
                    let _ = write_frame(&mut conn.writer, &ToHost::Bye).await;
                    break String::new();
                }
            },
            frame = conn.frames.recv() => match frame {
                Some(ToGuest::Bye { why }) => break why,
                Some(ToGuest::Ping) => heard = tokio::time::Instant::now(),
                Some(m) => {
                    heard = tokio::time::Instant::now();
                    let _ = inn.send(NetIn::FromHost(m));
                }
                None => break "Lost the connection to the host.".into(),
            },
            _ = ping.tick() => {
                if heard.elapsed() > SILENCE {
                    break "The host stopped answering.".into();
                }
                if write_frame(&mut conn.writer, &ToHost::Ping).await.is_err() {
                    break "Lost the connection to the host.".into();
                }
            }
        }
    };
    conn.reader.abort();
    let _ = conn.writer.shutdown().await;
    why
}

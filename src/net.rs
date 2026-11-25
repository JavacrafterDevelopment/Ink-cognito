use crate::protocol::{
    ClientDatagram, ClientMessage, GAME_PORT, HitEvent, MatchState, PING_EVERY, PlayerId,
    PlayerSnapshot, PlayerState, RagdollFrame, SILENCE_TIMEOUT, ServerDatagram, ServerMessage,
    encode_server_datagrams, enemies, player_color, read_frame, spawn_position, write_frame,
};
use bevy::prelude::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use tailcat::{Client, Region, Server, ServerConfig, TcpHandler};
use tokio::{
    io::DuplexStream,
    net::UdpSocket,
    runtime::Runtime,
    sync::{Mutex as AsyncMutex, Notify, mpsc},
};

/// How long a UDP peer can go silent before it's counted as gone.
const UDP_TIMEOUT: Duration = SILENCE_TIMEOUT;
/// How long goodbyes get to go out before the game closes or a host's tasks stop.
const GOODBYE_GRACE: Duration = Duration::from_millis(400);
/// How long a joining player keeps saying hello before giving up. Portside's
/// tunnel can take a few seconds to come up, and drops packets until it has.
const UDP_HELLO_FOR: Duration = Duration::from_secs(25);
/// Target tick interval for transform/world-state replication (50 Hz).
const NET_TICK: Duration = Duration::from_millis(20);

/// How this game reaches the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A pasted `tc...` address (TailCat, TCP).
    Tailcat,
    /// A session in the Portside hub (UDP through Portside's VOIP TailCat).
    Portside,
}

/// Whether the local player is hosting a Tailcat server or connected as a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRole {
    Host,
    Client,
}

/// Commands sent from the Bevy game loop to the background networking tasks.
#[derive(Debug)]
enum OutboundCommand {
    UpdateLocalTransform {
        position: [f32; 3],
        yaw: f32,
        state: PlayerState,
    },
    ReportHit(HitEvent),
    /// Host only: the match clock and scores, for everyone.
    Match(Box<MatchState>),
    /// Host only: the bodies' poses, for everyone.
    Ragdolls(Vec<RagdollFrame>),
}

/// Events produced by the background Tailcat networking tasks for Bevy to process.
#[derive(Debug, Clone)]
pub enum NetEvent {
    /// The game is up; Portside players can reach it on `udp_port` (127.0.0.1).
    HostingStarted {
        udp_port: u16,
        local_player: PlayerSnapshot,
    },
    /// The TailCat address others can paste is ready.
    TailcatReady(String),
    /// TailCat couldn't start: only Portside players can join.
    TailcatFailed(String),
    Connected {
        your_id: PlayerId,
        players: Vec<PlayerSnapshot>,
        match_state: Option<MatchState>,
    },
    PlayerJoined(PlayerSnapshot),
    PlayerLeft {
        id: PlayerId,
    },
    WorldState {
        players: Vec<PlayerSnapshot>,
    },
    /// Some players' state (no one is removed).
    PlayerUpdates(Vec<PlayerSnapshot>),
    /// Everyone in the game: anyone else is gone.
    Roster(Vec<PlayerId>),
    /// A shooter-confirmed hit on the local player.
    HitReceived(HitEvent),
    /// The host's match clock and scores.
    Match(MatchState),
    /// The host's ragdolls.
    Ragdolls(RagdollFrame),
    Error(String),
    Disconnected(String),
}

struct ActiveSession {
    role: NetRole,
    tx_out: mpsc::UnboundedSender<OutboundCommand>,
    rx_in: Mutex<mpsc::UnboundedReceiver<NetEvent>>,
}

/// Bevy resource that owns the Tokio runtime and active Tailcat multiplayer session.
#[derive(Resource)]
pub struct NetworkState {
    runtime: Runtime,
    session: Option<ActiveSession>,
    pub role: Option<NetRole>,
    pub local_player_id: Option<PlayerId>,
    pub tailcat_addr: Option<String>,
    /// Why there's no TailCat address while hosting, if it failed.
    pub tailcat_error: Option<String>,
    /// While hosting: the UDP port Portside sessions forward to.
    pub udp_port: Option<u16>,
    pub transport: Option<Transport>,
    pub status_text: String,
    pub error_text: Option<String>,
    pub is_connecting: bool,
    pub players: HashMap<PlayerId, PlayerSnapshot>,
    /// Confirmed hits on the local player queued for combat to apply this frame.
    pub pending_hits: Vec<HitEvent>,
    /// The match: what the host decides (and, hosting, what we decide).
    pub match_state: MatchState,
    /// The host's ragdoll updates, queued for the ragdolls to follow this frame.
    pub pending_ragdolls: Vec<RagdollFrame>,
}

impl Default for NetworkState {
    fn default() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("tailcat-net")
            .build()
            .expect("failed to build Tokio runtime for Tailcat networking");

        Self {
            runtime,
            session: None,
            role: None,
            local_player_id: None,
            tailcat_addr: None,
            tailcat_error: None,
            udp_port: None,
            transport: None,
            status_text: String::new(),
            error_text: None,
            is_connecting: false,
            players: HashMap::new(),
            pending_hits: Vec::new(),
            match_state: MatchState::default(),
            pending_ragdolls: Vec::new(),
        }
    }
}

impl NetworkState {
    /// Starts a Tailcat host server using Tailcat's default DERP relay map.
    pub fn start_host(&mut self) {
        self.start_host_with_region(None);
    }

    /// Starts a Tailcat host server, optionally pinned to a specific DERP region (useful for tests).
    pub fn start_host_with_region(&mut self, region: Option<Region>) {
        self.disconnect();
        self.role = Some(NetRole::Host);
        self.is_connecting = true;
        self.error_text = None;
        self.tailcat_addr = None;
        self.status_text = "Starting game...".to_string();

        let (tx_out, rx_out) = mpsc::unbounded_channel();
        let (tx_in, rx_in) = mpsc::unbounded_channel();
        let shutdown = Arc::new(Notify::new());

        let shutdown_task = shutdown.clone();
        self.runtime.spawn(async move {
            run_host_session(region, rx_out, tx_in, shutdown_task).await;
        });

        self.session = Some(ActiveSession {
            role: NetRole::Host,
            tx_out,
            rx_in: Mutex::new(rx_in),
        });
    }

    /// Connects to a remote host using its Tailcat address (`tc...`).
    pub fn join_game(&mut self, tailcat_addr: String) {
        let trimmed = tailcat_addr.trim().to_string();
        if trimmed.is_empty() {
            self.error_text = Some("Please enter a Tailcat address.".to_string());
            return;
        }

        self.disconnect();
        self.role = Some(NetRole::Client);
        self.is_connecting = true;
        self.error_text = None;
        self.tailcat_addr = Some(trimmed.clone());
        self.transport = Some(Transport::Tailcat);
        self.status_text = "Connecting over Tailcat...".to_string();

        let (tx_out, rx_out) = mpsc::unbounded_channel();
        let (tx_in, rx_in) = mpsc::unbounded_channel();
        let shutdown = Arc::new(Notify::new());

        let shutdown_task = shutdown.clone();
        self.runtime.spawn(async move {
            run_client_session(trimmed, rx_out, tx_in, shutdown_task).await;
        });

        self.session = Some(ActiveSession {
            role: NetRole::Client,
            tx_out,
            rx_in: Mutex::new(rx_in),
        });
    }

    /// Shows "joining" while Portside opens the session's tunnel (then `join_udp`).
    pub fn begin_portside_join(&mut self, label: &str) {
        self.disconnect();
        self.role = Some(NetRole::Client);
        self.is_connecting = true;
        self.error_text = None;
        self.transport = Some(Transport::Portside);
        self.status_text = format!("Opening Portside's tunnel to {label}...");
    }

    /// Joins a game through Portside: its session forwards `127.0.0.1:port`
    /// (UDP) to the host's game.
    pub fn join_udp(&mut self, port: u16, label: &str) {
        self.disconnect();
        self.role = Some(NetRole::Client);
        self.is_connecting = true;
        self.error_text = None;
        self.transport = Some(Transport::Portside);
        self.status_text = format!("Joining {label}...");

        let (tx_out, rx_out) = mpsc::unbounded_channel();
        let (tx_in, rx_in) = mpsc::unbounded_channel();
        let shutdown = Arc::new(Notify::new());

        let shutdown_task = shutdown.clone();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        self.runtime.spawn(async move {
            run_udp_client_session(addr, rx_out, tx_in, shutdown_task).await;
        });

        self.session = Some(ActiveSession {
            role: NetRole::Client,
            tx_out,
            rx_in: Mutex::new(rx_in),
        });
    }

    /// Sends the local player's position, facing yaw and state to the network layer.
    pub fn send_local_transform(&self, position: [f32; 3], yaw: f32, state: PlayerState) {
        if let Some(session) = &self.session {
            let _ = session.tx_out.send(OutboundCommand::UpdateLocalTransform {
                position,
                yaw,
                state,
            });
        }
    }

    /// Reports a confirmed paintball/water hit on another player.
    pub fn send_hit(&self, hit: HitEvent) {
        if let Some(session) = &self.session {
            let _ = session.tx_out.send(OutboundCommand::ReportHit(hit));
        }
    }

    /// Host only: tells everyone the match clock, phase and scores.
    pub fn send_match(&self, state: &MatchState) {
        if let Some(session) = &self.session {
            let _ = session
                .tx_out
                .send(OutboundCommand::Match(Box::new(state.clone())));
        }
    }

    /// Host only: tells everyone where the ragdolls are.
    pub fn send_ragdolls(&self, frames: Vec<RagdollFrame>) {
        if frames.is_empty() {
            return;
        }
        if let Some(session) = &self.session {
            let _ = session.tx_out.send(OutboundCommand::Ragdolls(frames));
        }
    }

    /// Drains all pending network events produced by background tasks without blocking.
    pub fn poll_events(&self) -> Vec<NetEvent> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if let Ok(mut rx) = session.rx_in.lock() {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }
        events
    }

    /// Cleanly terminates any active Tailcat host or client session.
    pub fn disconnect(&mut self) {
        // Dropping the session closes its command channel: its task then says
        // goodbye (Leave / HostLeaving / Bye) and stops on its own.
        drop(self.session.take());
        self.role = None;
        self.local_player_id = None;
        self.tailcat_addr = None;
        self.tailcat_error = None;
        self.udp_port = None;
        self.transport = None;
        self.is_connecting = false;
        self.status_text.clear();
        self.players.clear();
        self.pending_hits.clear();
        self.pending_ragdolls.clear();
        self.match_state = MatchState::default();
        crate::protocol::set_free_for_all(false);
    }

    pub fn active_role(&self) -> Option<NetRole> {
        self.session.as_ref().map(|s| s.role).or(self.role)
    }
}

impl Drop for NetworkState {
    fn drop(&mut self) {
        // Closing the game: give the goodbyes a moment before the runtime stops,
        // so others see us leave at once instead of after a timeout.
        let had_session = self.session.is_some();
        self.disconnect();
        if had_session {
            std::thread::sleep(GOODBYE_GRACE * 2);
        }
    }
}

/// Bounded deduplicator for `(attacker, hit_id)` so redundant UDP hit sends never apply twice.
#[derive(Default)]
struct HitTracker {
    seen: HashSet<(PlayerId, u32)>,
    order: VecDeque<(PlayerId, u32)>,
}

impl HitTracker {
    const CAPACITY: usize = 512;

    /// Returns `true` if this `(attacker, hit_id)` has not been seen yet.
    fn record(&mut self, attacker: PlayerId, hit_id: u32) -> bool {
        let key = (attacker, hit_id);
        if !self.seen.insert(key) {
            return false;
        }
        self.order.push_back(key);
        while self.order.len() > Self::CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
}

struct HostSharedState {
    players: HashMap<PlayerId, PlayerSnapshot>,
    clients: HashMap<PlayerId, mpsc::UnboundedSender<ServerMessage>>,
    seen_hits: HitTracker,
    match_state: Option<MatchState>,
}

impl HostSharedState {
    fn broadcast(&mut self, msg: &ServerMessage) {
        let mut dead = Vec::new();
        for (&id, tx) in &self.clients {
            if tx.send(msg.clone()).is_err() {
                dead.push(id);
            }
        }
        for id in dead {
            self.clients.remove(&id);
        }
    }

    fn snapshot_list(&self) -> Vec<PlayerSnapshot> {
        self.players.values().copied().collect()
    }
}

/// Validates, deduplicates, and forwards a shooter-reported hit to its victim.
async fn relay_hit(
    sender_id: PlayerId,
    mut hit: HitEvent,
    shared: &Arc<AsyncMutex<HostSharedState>>,
    tx_in: &mpsc::UnboundedSender<NetEvent>,
) {
    hit.attacker = sender_id;
    if !enemies(sender_id, hit.victim) {
        return;
    }
    if !hit.damage.is_finite() || hit.damage <= 0.0 {
        return;
    }
    hit.damage = hit.damage.min(150.0);

    let mut guard = shared.lock().await;
    if !guard.seen_hits.record(hit.attacker, hit.hit_id) {
        return;
    }
    if hit.victim == 0 {
        let _ = tx_in.send(NetEvent::HitReceived(hit));
    } else if let Some(client_tx) = guard.clients.get(&hit.victim) {
        let _ = client_tx.send(ServerMessage::PlayerHit(hit));
    }
}

async fn run_host_session(
    region: Option<Region>,
    mut rx_out: mpsc::UnboundedReceiver<OutboundCommand>,
    tx_in: mpsc::UnboundedSender<NetEvent>,
    shutdown: Arc<Notify>,
) {
    let host_player = PlayerSnapshot {
        id: 0,
        position: spawn_position(0),
        yaw: 0.0,
        color: player_color(0),
        state: PlayerState::default(),
    };

    let shared = Arc::new(AsyncMutex::new(HostSharedState {
        players: HashMap::from([(0, host_player)]),
        clients: HashMap::new(),
        seen_hits: HitTracker::default(),
        match_state: None,
    }));
    let next_id = Arc::new(AtomicU32::new(1));

    // UDP first: it's instant, and it's what Portside sessions carry.
    let udp = match UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(sock) => Arc::new(sock),
        Err(err) => {
            let _ = tx_in.send(NetEvent::Error(format!("Failed to start the game: {err}")));
            return;
        }
    };
    let udp_port = udp.local_addr().map(|a| a.port()).unwrap_or(0);
    info!("Game listening for Portside players on 127.0.0.1:{udp_port} (UDP)");
    let udp_task = tokio::spawn(run_udp_host(
        udp,
        shared.clone(),
        next_id.clone(),
        tx_in.clone(),
    ));
    let _ = tx_in.send(NetEvent::HostingStarted {
        udp_port,
        local_player: host_player,
    });

    let shared_for_accept = shared.clone();
    let next_id_for_accept = next_id.clone();
    let tx_in_for_accept = tx_in.clone();
    let shutdown_for_accept = shutdown.clone();

    let tcp_handler: TcpHandler = Arc::new(move |stream: DuplexStream| {
        let shared = shared_for_accept.clone();
        let next_id = next_id_for_accept.clone();
        let tx_in = tx_in_for_accept.clone();
        let shutdown = shutdown_for_accept.clone();
        Box::pin(async move {
            handle_host_client(stream, shared, next_id, tx_in, shutdown).await;
        }) as Pin<Box<dyn Future<Output = ()> + Send>>
    });

    let config = ServerConfig {
        region,
        on_tcp: Some(Arc::new(move |port| {
            (port == GAME_PORT).then(|| tcp_handler.clone())
        })),
        ..Default::default()
    };

    let mut broadcast_tick = tokio::time::interval(NET_TICK);
    broadcast_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // TailCat (for a pasted address) can take a while; keep broadcasting to
    // Portside UDP players meanwhile so movement stays live from frame one.
    let start = Server::start(config);
    tokio::pin!(start);
    let server = loop {
        tokio::select! {
            _ = shutdown.notified() => {
                host_goodbye(&shared, &shutdown, udp_task).await;
                return;
            }
            cmd = rx_out.recv() => match cmd {
                Some(OutboundCommand::UpdateLocalTransform { position, yaw, state }) => {
                    set_host_transform(&shared, position, yaw, state).await;
                }
                Some(OutboundCommand::ReportHit(hit)) => {
                    relay_hit(0, hit, &shared, &tx_in).await;
                }
                Some(OutboundCommand::Match(state)) => {
                    set_match(&shared, *state).await;
                }
                Some(OutboundCommand::Ragdolls(frames)) => {
                    broadcast_ragdolls(&shared, frames).await;
                }
                None => {
                    host_goodbye(&shared, &shutdown, udp_task).await;
                    return;
                }
            },
            _ = broadcast_tick.tick() => {
                let mut guard = shared.lock().await;
                if !guard.clients.is_empty() {
                    let players = guard.snapshot_list();
                    guard.broadcast(&ServerMessage::WorldState { players });
                }
            }
            res = &mut start => break res,
        }
    };
    let server = match server {
        Ok(server) => {
            let tailcat_addr = server.tailcat_addr();
            info!("Tailcat host listening. Share this address to join:\n{tailcat_addr}");
            let _ = tx_in.send(NetEvent::TailcatReady(tailcat_addr));
            Some(server)
        }
        Err(err) => {
            let _ = tx_in.send(NetEvent::TailcatFailed(format!(
                "Tailcat address unavailable: {err}"
            )));
            None
        }
    };

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                break;
            }
            cmd = rx_out.recv() => {
                match cmd {
                    Some(OutboundCommand::UpdateLocalTransform { position, yaw, state }) => {
                        set_host_transform(&shared, position, yaw, state).await;
                    }
                    Some(OutboundCommand::ReportHit(hit)) => {
                        relay_hit(0, hit, &shared, &tx_in).await;
                    }
                    Some(OutboundCommand::Match(state)) => {
                        set_match(&shared, *state).await;
                    }
                    Some(OutboundCommand::Ragdolls(frames)) => {
                        broadcast_ragdolls(&shared, frames).await;
                    }
                    None => break,
                }
            }
            _ = broadcast_tick.tick() => {
                let mut guard = shared.lock().await;
                if !guard.clients.is_empty() {
                    let players = guard.snapshot_list();
                    guard.broadcast(&ServerMessage::WorldState { players });
                }
            }
        }
    }

    host_goodbye(&shared, &shutdown, udp_task).await;
    if let Some(server) = server {
        server.close().await;
    }
}

/// The host is closing: tell every player, give it a moment to go out, then
/// stop each player's task.
async fn host_goodbye(
    shared: &Arc<AsyncMutex<HostSharedState>>,
    shutdown: &Notify,
    udp_task: tokio::task::JoinHandle<()>,
) {
    {
        let mut guard = shared.lock().await;
        guard.broadcast(&ServerMessage::HostLeaving);
        // Dropping each player's sender ends its writer once HostLeaving is
        // out (and says Bye to the UDP ones).
        guard.clients.clear();
    }
    tokio::time::sleep(GOODBYE_GRACE).await;
    shutdown.notify_waiters();
    udp_task.abort();
}

/// Remembers the match (for players who join later) and tells everyone.
async fn set_match(shared: &Arc<AsyncMutex<HostSharedState>>, state: MatchState) {
    let mut guard = shared.lock().await;
    guard.broadcast(&ServerMessage::Match(state.clone()));
    guard.match_state = Some(state);
}

/// Sends the host's ragdolls to every player.
async fn broadcast_ragdolls(shared: &Arc<AsyncMutex<HostSharedState>>, frames: Vec<RagdollFrame>) {
    let mut guard = shared.lock().await;
    if guard.clients.is_empty() {
        return;
    }
    for frame in frames {
        guard.broadcast(&ServerMessage::Ragdolls(frame));
    }
}

async fn set_host_transform(
    shared: &Arc<AsyncMutex<HostSharedState>>,
    position: [f32; 3],
    yaw: f32,
    state: PlayerState,
) {
    let mut guard = shared.lock().await;
    if let Some(host) = guard.players.get_mut(&0) {
        host.position = position;
        host.yaw = yaw;
        host.state = state;
    }
}

struct UdpPeer {
    id: PlayerId,
    last_seen: Instant,
    welcome: Vec<Vec<u8>>,
}

/// The host's UDP side: each player is the address its packets come from
/// (through Portside every joiner is 127.0.0.1 with a port of its own).
async fn run_udp_host(
    sock: Arc<UdpSocket>,
    shared: Arc<AsyncMutex<HostSharedState>>,
    next_id: Arc<AtomicU32>,
    tx_in: mpsc::UnboundedSender<NetEvent>,
) {
    let mut peers: HashMap<SocketAddr, UdpPeer> = HashMap::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut sweep = tokio::time::interval(Duration::from_secs(1));
    sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            res = sock.recv_from(&mut buf) => {
                let Ok((len, from)) = res else {
                    // ICMP "port unreachable" from a gone peer shows up as an error on some OSes.
                    continue;
                };
                let Ok(msg) = serde_json::from_slice::<ClientDatagram>(&buf[..len]) else {
                    continue;
                };
                if let Some(peer) = peers.get_mut(&from) {
                    peer.last_seen = Instant::now();
                }
                match msg {
                    ClientDatagram::Hello => {
                        if let Some(peer) = peers.get(&from) {
                            // Our welcome was lost: say it again.
                            for d in &peer.welcome {
                                let _ = sock.send_to(d, from).await;
                            }
                            continue;
                        }
                        let id = next_id.fetch_add(1, Ordering::Relaxed);
                        let snapshot = PlayerSnapshot {
                            id,
                            position: spawn_position(id),
                            yaw: 0.0,
                            color: player_color(id),
                            state: PlayerState::default(),
                        };
                        let (client_tx, mut client_rx) = mpsc::unbounded_channel::<ServerMessage>();
                        let welcome = {
                            let mut guard = shared.lock().await;
                            guard.broadcast(&ServerMessage::PlayerJoined(snapshot));
                            guard.players.insert(id, snapshot);
                            guard.clients.insert(id, client_tx);
                            ServerMessage::Welcome {
                                your_id: id,
                                players: guard.snapshot_list(),
                                match_state: guard.match_state.clone(),
                            }
                        };
                        let welcome = encode_server_datagrams(&welcome);
                        for d in &welcome {
                            let _ = sock.send_to(d, from).await;
                        }
                        peers.insert(from, UdpPeer { id, last_seen: Instant::now(), welcome });
                        let _ = tx_in.send(NetEvent::PlayerJoined(snapshot));

                        let sock = sock.clone();
                        tokio::spawn(async move {
                            while let Some(msg) = client_rx.recv().await {
                                let is_hit = matches!(msg, ServerMessage::PlayerHit(_));
                                let datagrams = encode_server_datagrams(&msg);
                                for d in &datagrams {
                                    let _ = sock.send_to(d, from).await;
                                }
                                if is_hit {
                                    let sock_retry = sock.clone();
                                    tokio::spawn(async move {
                                        for delay_ms in [35u64, 40] {
                                            tokio::time::sleep(Duration::from_millis(delay_ms))
                                                .await;
                                            for d in &datagrams {
                                                let _ = sock_retry.send_to(d, from).await;
                                            }
                                        }
                                    });
                                }
                            }
                            if let Ok(bye) = serde_json::to_vec(&ServerDatagram::Bye) {
                                let _ = sock.send_to(&bye, from).await;
                            }
                        });
                    }
                    ClientDatagram::Msg(ClientMessage::UpdateTransform { position, yaw, state }) => {
                        let Some(peer) = peers.get(&from) else {
                            // A player we dropped (or never met): ask it to join again.
                            if let Ok(bye) = serde_json::to_vec(&ServerDatagram::Bye) {
                                let _ = sock.send_to(&bye, from).await;
                            }
                            continue;
                        };
                        let players = {
                            let mut guard = shared.lock().await;
                            if let Some(player) = guard.players.get_mut(&peer.id) {
                                player.position = position;
                                player.yaw = yaw;
                                player.state = state;
                            }
                            guard.snapshot_list()
                        };
                        let _ = tx_in.send(NetEvent::WorldState { players });
                    }
                    ClientDatagram::Msg(ClientMessage::ReportHit(hit)) => {
                        if let Some(peer) = peers.get(&from) {
                            relay_hit(peer.id, hit, &shared, &tx_in).await;
                        }
                    }
                    ClientDatagram::Msg(ClientMessage::Ping) => {}
                    ClientDatagram::Msg(ClientMessage::Leave) => {
                        if let Some(peer) = peers.remove(&from) {
                            cleanup_host_client(peer.id, &shared, &tx_in).await;
                        }
                    }
                    ClientDatagram::Keepalive => {
                        if peers.contains_key(&from)
                            && let Ok(d) = serde_json::to_vec(&ServerDatagram::Keepalive)
                        {
                            let _ = sock.send_to(&d, from).await;
                        }
                    }
                    ClientDatagram::Bye => {
                        if let Some(peer) = peers.remove(&from) {
                            cleanup_host_client(peer.id, &shared, &tx_in).await;
                        }
                    }
                }
            }
            _ = sweep.tick() => {
                let gone: Vec<SocketAddr> = peers
                    .iter()
                    .filter(|(_, p)| p.last_seen.elapsed() > UDP_TIMEOUT)
                    .map(|(a, _)| *a)
                    .collect();
                for addr in gone {
                    if let Some(peer) = peers.remove(&addr) {
                        cleanup_host_client(peer.id, &shared, &tx_in).await;
                    }
                }
            }
        }
    }
}

async fn handle_host_client(
    stream: DuplexStream,
    shared: Arc<AsyncMutex<HostSharedState>>,
    next_id: Arc<AtomicU32>,
    tx_in: mpsc::UnboundedSender<NetEvent>,
    shutdown: Arc<Notify>,
) {
    let player_id = next_id.fetch_add(1, Ordering::Relaxed);
    let snapshot = PlayerSnapshot {
        id: player_id,
        position: spawn_position(player_id),
        yaw: 0.0,
        color: player_color(player_id),
        state: PlayerState::default(),
    };

    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<ServerMessage>();

    let welcome = {
        let mut guard = shared.lock().await;
        guard.broadcast(&ServerMessage::PlayerJoined(snapshot));
        guard.players.insert(player_id, snapshot);
        guard.clients.insert(player_id, client_tx.clone());
        ServerMessage::Welcome {
            your_id: player_id,
            players: guard.snapshot_list(),
            match_state: guard.match_state.clone(),
        }
    };
    // Welcome goes first, through the same writer as everything else.
    let _ = client_tx.send(welcome);
    drop(client_tx);

    let _ = tx_in.send(NetEvent::PlayerJoined(snapshot));

    let (reader, mut writer) = tokio::io::split(stream);

    // Writes until the host drops this player's sender (left, or the host closing).
    let writer_task = tokio::spawn(async move {
        while let Some(msg) = client_rx.recv().await {
            if write_frame(&mut writer, &msg).await.is_err() {
                break;
            }
        }
    });
    // Reads on a task of its own: a frame read must never be cut off halfway.
    let (mut frames, reader_task) = spawn_frame_reader::<ClientMessage, _>(reader);

    let mut last_heard = Instant::now();
    let mut check = tokio::time::interval(Duration::from_secs(1));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            _ = check.tick() => {
                if last_heard.elapsed() > SILENCE_TIMEOUT {
                    info!("Player {player_id} stopped responding; dropping them.");
                    break;
                }
            }
            frame = frames.recv() => {
                last_heard = Instant::now();
                match frame {
                    Some(ClientMessage::UpdateTransform { position, yaw, state }) => {
                        let players = {
                            let mut guard = shared.lock().await;
                            if let Some(player) = guard.players.get_mut(&player_id) {
                                player.position = position;
                                player.yaw = yaw;
                                player.state = state;
                            }
                            guard.snapshot_list()
                        };
                        let _ = tx_in.send(NetEvent::WorldState { players });
                    }
                    Some(ClientMessage::ReportHit(hit)) => {
                        relay_hit(player_id, hit, &shared, &tx_in).await;
                    }
                    Some(ClientMessage::Ping) => {}
                    Some(ClientMessage::Leave) | None => break,
                }
            }
        }
    }

    reader_task.abort();
    cleanup_host_client(player_id, &shared, &tx_in).await;
    // Its sender is gone now: the writer finishes what's queued, or gives up.
    if tokio::time::timeout(GOODBYE_GRACE, writer_task)
        .await
        .is_err()
    {
        warn!("Player {player_id}'s connection didn't take the last messages.");
    }
}

/// Reads frames on a task of its own and hands them over a channel, which
/// closes on end of stream or a bad frame. (Reading inside a `select!` with
/// timers can drop a half-read frame and garble the stream.)
fn spawn_frame_reader<T, R>(
    mut reader: R,
) -> (mpsc::UnboundedReceiver<T>, tokio::task::JoinHandle<()>)
where
    T: serde::de::DeserializeOwned + Send + 'static,
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Ok(Some(msg)) = read_frame::<_, T>(&mut reader).await {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    (rx, task)
}

async fn cleanup_host_client(
    player_id: PlayerId,
    shared: &Arc<AsyncMutex<HostSharedState>>,
    tx_in: &mpsc::UnboundedSender<NetEvent>,
) {
    let mut guard = shared.lock().await;
    guard.clients.remove(&player_id);
    if guard.players.remove(&player_id).is_some() {
        guard.broadcast(&ServerMessage::PlayerLeft { id: player_id });
        let _ = tx_in.send(NetEvent::PlayerLeft { id: player_id });
    }
}

async fn run_client_session(
    tailcat_addr: String,
    mut rx_out: mpsc::UnboundedReceiver<OutboundCommand>,
    tx_in: mpsc::UnboundedSender<NetEvent>,
    shutdown: Arc<Notify>,
) {
    let client = match Client::connect(&tailcat_addr, None, None).await {
        Ok(client) => client,
        Err(err) => {
            let _ = tx_in.send(NetEvent::Error(format!(
                "Could not connect via Tailcat: {err}"
            )));
            return;
        }
    };

    let stream = match client.dial_tcp_port(GAME_PORT).await {
        Ok(stream) => stream,
        Err(err) => {
            client.close().await;
            let _ = tx_in.send(NetEvent::Error(format!(
                "Connected to peer, but game stream failed: {err}"
            )));
            return;
        }
    };

    let (reader, mut writer) = tokio::io::split(stream);
    let (mut frames, reader_task) = spawn_frame_reader::<ServerMessage, _>(reader);

    // Wait for the Welcome from the host.
    let welcome = tokio::select! {
        frame = tokio::time::timeout(Duration::from_secs(20), frames.recv()) => frame,
        _ = rx_out.recv() => {
            reader_task.abort();
            client.close().await;
            return;
        }
    };
    match welcome {
        Ok(Some(ServerMessage::Welcome {
            your_id,
            players,
            match_state,
        })) => {
            let _ = tx_in.send(NetEvent::Connected {
                your_id,
                players,
                match_state,
            });
        }
        Ok(Some(_)) => {
            reader_task.abort();
            client.close().await;
            let _ = tx_in.send(NetEvent::Error(
                "Unexpected handshake message from host.".to_string(),
            ));
            return;
        }
        Ok(None) => {
            client.close().await;
            let _ = tx_in.send(NetEvent::Error(
                "Host closed the connection during handshake.".to_string(),
            ));
            return;
        }
        Err(_) => {
            reader_task.abort();
            client.close().await;
            let _ = tx_in.send(NetEvent::Error("The host didn't answer.".to_string()));
            return;
        }
    }

    let mut latest_transform: Option<([f32; 3], f32, PlayerState)> = None;
    let mut last_sent_shots = 0u32;
    let mut last_sent_deaths = 0u32;
    let mut send_tick = tokio::time::interval(NET_TICK);
    send_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ping_tick = tokio::time::interval(PING_EVERY);
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_heard = Instant::now();

    let reason = loop {
        tokio::select! {
            _ = shutdown.notified() => break None,
            cmd = rx_out.recv() => {
                match cmd {
                    Some(OutboundCommand::UpdateLocalTransform { position, yaw, state }) => {
                        let urgent = state.shots != last_sent_shots || state.deaths != last_sent_deaths;
                        if urgent {
                            last_sent_shots = state.shots;
                            last_sent_deaths = state.deaths;
                            latest_transform = None;
                            if write_frame(&mut writer, &ClientMessage::UpdateTransform { position, yaw, state })
                                .await
                                .is_err()
                            {
                                break Some("Lost connection to host.".to_string());
                            }
                        } else {
                            latest_transform = Some((position, yaw, state));
                        }
                    }
                    Some(OutboundCommand::ReportHit(hit)) => {
                        if write_frame(&mut writer, &ClientMessage::ReportHit(hit))
                            .await
                            .is_err()
                        {
                            break Some("Lost connection to host.".to_string());
                        }
                    }
                    Some(OutboundCommand::Match(_) | OutboundCommand::Ragdolls(_)) => {}
                    None => break None,
                }
            }
            _ = send_tick.tick() => {
                if let Some((position, yaw, state)) = latest_transform.take() {
                    last_sent_shots = state.shots;
                    last_sent_deaths = state.deaths;
                    if write_frame(&mut writer, &ClientMessage::UpdateTransform { position, yaw, state })
                        .await
                        .is_err()
                    {
                        break Some("Lost connection to host.".to_string());
                    }
                }
            }
            _ = ping_tick.tick() => {
                if last_heard.elapsed() > SILENCE_TIMEOUT {
                    break Some("The host stopped responding.".to_string());
                }
                if write_frame(&mut writer, &ClientMessage::Ping).await.is_err() {
                    break Some("Lost connection to host.".to_string());
                }
            }
            frame = frames.recv() => {
                last_heard = Instant::now();
                match frame {
                    Some(ServerMessage::PlayerJoined(player)) => {
                        let _ = tx_in.send(NetEvent::PlayerJoined(player));
                    }
                    Some(ServerMessage::PlayerLeft { id }) => {
                        let _ = tx_in.send(NetEvent::PlayerLeft { id });
                    }
                    Some(ServerMessage::WorldState { players }) => {
                        let _ = tx_in.send(NetEvent::WorldState { players });
                    }
                    Some(ServerMessage::PlayerHit(hit)) => {
                        let _ = tx_in.send(NetEvent::HitReceived(hit));
                    }
                    Some(ServerMessage::Match(state)) => {
                        let _ = tx_in.send(NetEvent::Match(state));
                    }
                    Some(ServerMessage::Ragdolls(frame)) => {
                        let _ = tx_in.send(NetEvent::Ragdolls(frame));
                    }
                    Some(ServerMessage::Welcome { your_id, players, match_state }) => {
                        let _ = tx_in.send(NetEvent::Connected { your_id, players, match_state });
                    }
                    Some(ServerMessage::HostLeaving) | None => {
                        break Some("Host ended the game.".to_string());
                    }
                }
            }
        }
    };

    if reason.is_none() {
        // We're the one leaving: say so, so the host drops us right away.
        let _ = tokio::time::timeout(
            GOODBYE_GRACE,
            write_frame(&mut writer, &ClientMessage::Leave),
        )
        .await;
        // The tunnel sends in the background: let it go out before closing.
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    reader_task.abort();
    client.close().await;
    if let Some(reason) = reason {
        let _ = tx_in.send(NetEvent::Disconnected(reason));
    }
}

/// A player joining through Portside: everything is UDP to `127.0.0.1:port`,
/// which Portside carries to the host's game.
async fn run_udp_client_session(
    host: SocketAddr,
    mut rx_out: mpsc::UnboundedReceiver<OutboundCommand>,
    tx_in: mpsc::UnboundedSender<NetEvent>,
    shutdown: Arc<Notify>,
) {
    let sock = match UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(sock) => Arc::new(sock),
        Err(err) => {
            let _ = tx_in.send(NetEvent::Error(format!(
                "Could not open a UDP socket: {err}"
            )));
            return;
        }
    };
    if let Err(err) = sock.connect(host).await {
        let _ = tx_in.send(NetEvent::Error(format!(
            "Could not reach Portside's session: {err}"
        )));
        return;
    }
    let send = |msg: &ClientDatagram| serde_json::to_vec(msg).unwrap_or_default();
    let hello = send(&ClientDatagram::Hello);
    let mut buf = vec![0u8; 64 * 1024];

    // Say hello until the host answers: the tunnel drops packets until it's up.
    let started = Instant::now();
    let mut retry = tokio::time::interval(Duration::from_millis(250));
    let mut welcome_id: Option<PlayerId> = None;
    let mut welcome_players: Vec<PlayerSnapshot> = Vec::new();
    let mut welcome_match: Option<MatchState> = None;
    loop {
        tokio::select! {
            _ = shutdown.notified() => return,
            cmd = rx_out.recv() => if cmd.is_none() { return },
            _ = retry.tick() => {
                if welcome_id.is_some() {
                    break;
                }
                if started.elapsed() > UDP_HELLO_FOR {
                    let _ = tx_in.send(NetEvent::Error(
                        "The host didn't answer. They may have left, or the hub's tunnel is down.".to_string(),
                    ));
                    return;
                }
                let _ = sock.send(&hello).await;
            }
            res = sock.recv(&mut buf) => {
                let Ok(len) = res else { continue };
                match serde_json::from_slice::<ServerDatagram>(&buf[..len]) {
                    Ok(ServerDatagram::Msg(ServerMessage::Welcome { your_id, players, match_state })) => {
                        welcome_id = Some(your_id);
                        welcome_players = players;
                        welcome_match = match_state;
                    }
                    // A big world comes as Welcome then chunks, right behind it.
                    Ok(ServerDatagram::Players { players }) if welcome_id.is_some() => {
                        welcome_players.extend(players);
                    }
                    _ => {}
                }
            }
        }
    }
    let _ = tx_in.send(NetEvent::Connected {
        your_id: welcome_id.unwrap_or_default(),
        players: welcome_players,
        match_state: welcome_match,
    });

    let mut latest_transform: Option<([f32; 3], f32, PlayerState)> = None;
    let mut last_sent_shots = 0u32;
    let mut last_sent_deaths = 0u32;
    let mut send_tick = tokio::time::interval(NET_TICK);
    send_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut keepalive = tokio::time::interval(Duration::from_secs(2));
    let mut last_heard = Instant::now();

    let reason = loop {
        tokio::select! {
            _ = shutdown.notified() => break None,
            cmd = rx_out.recv() => match cmd {
                Some(OutboundCommand::UpdateLocalTransform { position, yaw, state }) => {
                    let urgent = state.shots != last_sent_shots || state.deaths != last_sent_deaths;
                    if urgent {
                        last_sent_shots = state.shots;
                        last_sent_deaths = state.deaths;
                        latest_transform = None;
                        let msg = ClientDatagram::Msg(ClientMessage::UpdateTransform { position, yaw, state });
                        let _ = sock.send(&send(&msg)).await;
                    } else {
                        latest_transform = Some((position, yaw, state));
                    }
                }
                Some(OutboundCommand::ReportHit(hit)) => {
                    let packet = send(&ClientDatagram::Msg(ClientMessage::ReportHit(hit)));
                    let _ = sock.send(&packet).await;
                    let sock_retry = sock.clone();
                    tokio::spawn(async move {
                        for delay_ms in [35u64, 40] {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            let _ = sock_retry.send(&packet).await;
                        }
                    });
                }
                Some(OutboundCommand::Match(_) | OutboundCommand::Ragdolls(_)) => {}
                None => break None,
            },
            _ = send_tick.tick() => {
                if let Some((position, yaw, state)) = latest_transform.take() {
                    last_sent_shots = state.shots;
                    last_sent_deaths = state.deaths;
                    let msg = ClientDatagram::Msg(ClientMessage::UpdateTransform { position, yaw, state });
                    let _ = sock.send(&send(&msg)).await;
                }
            }
            _ = keepalive.tick() => {
                if last_heard.elapsed() > UDP_TIMEOUT {
                    break Some("Lost connection to host.".to_string());
                }
                let _ = sock.send(&send(&ClientDatagram::Keepalive)).await;
            }
            res = sock.recv(&mut buf) => {
                let Ok(len) = res else { continue };
                let Ok(msg) = serde_json::from_slice::<ServerDatagram>(&buf[..len]) else { continue };
                last_heard = Instant::now();
                let event = match msg {
                    ServerDatagram::Msg(ServerMessage::PlayerJoined(p)) => NetEvent::PlayerJoined(p),
                    ServerDatagram::Msg(ServerMessage::PlayerLeft { id }) => NetEvent::PlayerLeft { id },
                    ServerDatagram::Msg(ServerMessage::WorldState { players }) => NetEvent::WorldState { players },
                    ServerDatagram::Msg(ServerMessage::PlayerHit(hit)) => NetEvent::HitReceived(hit),
                    ServerDatagram::Msg(ServerMessage::Match(state)) => NetEvent::Match(state),
                    ServerDatagram::Msg(ServerMessage::Ragdolls(frame)) => NetEvent::Ragdolls(frame),
                    ServerDatagram::Msg(ServerMessage::Welcome { .. }) => continue,
                    ServerDatagram::Players { players } => NetEvent::PlayerUpdates(players),
                    ServerDatagram::Roster { ids } => NetEvent::Roster(ids),
                    ServerDatagram::Keepalive => continue,
                    ServerDatagram::Msg(ServerMessage::HostLeaving) | ServerDatagram::Bye => {
                        break Some("Host ended the game.".to_string())
                    }
                };
                let _ = tx_in.send(event);
            }
        }
    };
    // Tell the host now rather than letting it time us out.
    let bye = send(&ClientDatagram::Bye);
    let _ = sock.send(&bye).await;
    let _ = sock.send(&bye).await;
    if let Some(reason) = reason {
        let _ = tx_in.send(NetEvent::Disconnected(reason));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailcat_host_and_client_sync_positions() {
        let mut host = NetworkState::default();
        let (region, _relay) = host
            .runtime
            .block_on(tailcat::derp::start_local_relay())
            .expect("failed to start local DERP relay");

        host.start_host_with_region(Some(region));

        // Wait for host address.
        let mut addr = None;
        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::TailcatReady(tailcat_addr) = ev {
                    addr = Some(tailcat_addr);
                }
            }
            if addr.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let addr = addr.expect("host did not emit HostingStarted");
        assert!(addr.starts_with("tc"));

        let mut client = NetworkState::default();
        client.join_game(addr.clone());

        let mut client_id = None;
        for _ in 0..200 {
            for ev in client.poll_events() {
                if let NetEvent::Connected {
                    your_id, players, ..
                } = ev
                {
                    assert!(players.iter().any(|p| p.id == 0));
                    client_id = Some(your_id);
                }
            }
            if client_id.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let client_id = client_id.expect("client did not connect");
        assert_eq!(client_id, 1);

        // Move both players and verify transforms sync in both directions.
        host.send_local_transform([4.0, 1.0, -2.5], 1.25, PlayerState::default());
        client.send_local_transform([-3.0, 1.0, 5.5], -0.75, PlayerState::default());

        let mut host_saw_client = false;
        let mut client_saw_host = false;

        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::WorldState { players } = ev
                    && players
                        .iter()
                        .any(|p| p.id == client_id && (p.position[0] + 3.0).abs() < 0.01)
                {
                    host_saw_client = true;
                }
            }
            for ev in client.poll_events() {
                if let NetEvent::WorldState { players } = ev
                    && players
                        .iter()
                        .any(|p| p.id == 0 && (p.position[0] - 4.0).abs() < 0.01)
                {
                    client_saw_host = true;
                }
            }
            if host_saw_client && client_saw_host {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }

        assert!(host_saw_client, "host should receive client transform");
        assert!(client_saw_host, "client should receive host transform");

        // Leaving tells the host at once.
        client.disconnect();
        let t = Instant::now();
        assert!(
            wait_for(&host, Duration::from_secs(3), |e| matches!(
                e,
                NetEvent::PlayerLeft { id } if *id == client_id
            )),
            "host should see the player leave"
        );
        eprintln!("host saw the player leave in {:?}", t.elapsed());

        // The host closing sends everyone back to the menu at once.
        let mut second = NetworkState::default();
        second.join_game(addr);
        assert!(wait_for(&second, Duration::from_secs(10), |e| matches!(
            e,
            NetEvent::Connected { .. }
        )));
        host.disconnect();
        assert!(
            wait_for(&second, Duration::from_secs(3), |e| matches!(
                e,
                NetEvent::Disconnected(_)
            )),
            "client should hear the host ended the game"
        );
    }

    /// Waits for an event matching `want`, up to `limit`.
    fn wait_for(net: &NetworkState, limit: Duration, want: impl Fn(&NetEvent) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if net.poll_events().iter().any(&want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// Stops a game's networking dead, as a crash or Alt+F4 would: no goodbyes.
    fn vanish(net: &mut NetworkState) {
        let rt = std::mem::replace(
            &mut net.runtime,
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        rt.shutdown_background();
        net.session = None;
    }

    /// Someone whose game vanished is noticed within 15 seconds, both ways (TailCat).
    #[test]
    fn tailcat_vanished_player_and_host_are_noticed() {
        let relay_rt = tokio::runtime::Runtime::new().unwrap();
        let (region, _relay) = relay_rt
            .block_on(tailcat::derp::start_local_relay())
            .expect("failed to start local DERP relay");

        let mut host = NetworkState::default();
        host.start_host_with_region(Some(region));
        let mut addr = None;
        let start = Instant::now();
        while addr.is_none() && start.elapsed() < Duration::from_secs(10) {
            for ev in host.poll_events() {
                if let NetEvent::TailcatReady(a) = ev {
                    addr = Some(a);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let addr = addr.expect("no TailCat address");

        let mut client = NetworkState::default();
        client.join_game(addr.clone());
        assert!(wait_for(&client, Duration::from_secs(10), |e| matches!(
            e,
            NetEvent::Connected { .. }
        )));
        vanish(&mut client);
        let t = Instant::now();
        assert!(
            wait_for(&host, Duration::from_secs(15), |e| matches!(
                e,
                NetEvent::PlayerLeft { .. }
            )),
            "host should drop a player that vanished"
        );
        eprintln!("host noticed the vanished player in {:?}", t.elapsed());

        let mut client = NetworkState::default();
        client.join_game(addr);
        assert!(wait_for(&client, Duration::from_secs(10), |e| matches!(
            e,
            NetEvent::Connected { .. }
        )));
        vanish(&mut host);
        let t = Instant::now();
        assert!(
            wait_for(&client, Duration::from_secs(15), |e| matches!(
                e,
                NetEvent::Disconnected(_)
            )),
            "client should notice the host vanished"
        );
        eprintln!("client noticed the vanished host in {:?}", t.elapsed());
    }

    /// The same over UDP (Portside): silent peers are dropped within 15 seconds.
    #[test]
    fn udp_vanished_player_and_host_are_noticed() {
        // A player that says hello and then goes silent.
        let mut host = NetworkState::default();
        host.start_host_with_region(None);
        let mut port = None;
        let start = Instant::now();
        while port.is_none() && start.elapsed() < Duration::from_secs(5) {
            for ev in host.poll_events() {
                if let NetEvent::HostingStarted { udp_port, .. } = ev {
                    port = Some(udp_port);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let port = port.expect("host didn't start");
        let mute = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        mute.send_to(
            &serde_json::to_vec(&ClientDatagram::Hello).unwrap(),
            ("127.0.0.1", port),
        )
        .unwrap();
        assert!(wait_for(&host, Duration::from_secs(3), |e| matches!(
            e,
            NetEvent::PlayerJoined(_)
        )));
        let t = Instant::now();
        assert!(
            wait_for(&host, Duration::from_secs(15), |e| matches!(
                e,
                NetEvent::PlayerLeft { .. }
            )),
            "host should drop a silent UDP player"
        );
        eprintln!("host noticed the silent UDP player in {:?}", t.elapsed());

        // A host that welcomes us and then goes silent.
        let fake_host = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let fake_port = fake_host.local_addr().unwrap().port();
        let mut client = NetworkState::default();
        client.join_udp(fake_port, "test");
        let mut buf = [0u8; 2048];
        let (_, from) = fake_host.recv_from(&mut buf).unwrap();
        let welcome = ServerDatagram::Msg(ServerMessage::Welcome {
            your_id: 1,
            players: vec![],
            match_state: None,
        });
        fake_host
            .send_to(&serde_json::to_vec(&welcome).unwrap(), from)
            .unwrap();
        assert!(wait_for(&client, Duration::from_secs(3), |e| matches!(
            e,
            NetEvent::Connected { .. }
        )));
        let t = Instant::now();
        assert!(
            wait_for(&client, Duration::from_secs(15), |e| matches!(
                e,
                NetEvent::Disconnected(_)
            )),
            "client should notice the host went silent"
        );
        eprintln!("client noticed the silent UDP host in {:?}", t.elapsed());
    }

    /// The way a Portside session reaches the game: UDP to a port on 127.0.0.1.
    /// (Portside forwards that port to the host's; here the client uses the host's directly.)
    #[test]
    fn udp_host_and_client_sync_positions() {
        let mut host = NetworkState::default();
        // TailCat may or may not start in here: the UDP side doesn't need it.
        host.start_host_with_region(None);

        let mut port = None;
        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::HostingStarted { udp_port, .. } = ev {
                    port = Some(udp_port);
                }
            }
            if port.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let port = port.expect("host did not emit HostingStarted");

        let mut clients = Vec::new();
        for n in 0..2 {
            let mut client = NetworkState::default();
            client.join_udp(port, "test");
            let mut id = None;
            for _ in 0..200 {
                for ev in client.poll_events() {
                    if let NetEvent::Connected {
                        your_id, players, ..
                    } = ev
                    {
                        assert!(players.iter().any(|p| p.id == 0));
                        id = Some(your_id);
                    }
                }
                if id.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(id, Some(n + 1), "client {n} did not get its welcome");
            clients.push(client);
        }

        host.send_local_transform([4.0, 1.0, -2.5], 1.25, PlayerState::default());
        clients[0].send_local_transform([-3.0, 1.0, 5.5], -0.75, PlayerState::default());

        let mut host_saw_client = false;
        let mut other_saw_client = false;
        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::WorldState { players } = ev
                    && players
                        .iter()
                        .any(|p| p.id == 1 && (p.position[0] + 3.0).abs() < 0.01)
                {
                    host_saw_client = true;
                }
            }
            for ev in clients[1].poll_events() {
                if let NetEvent::WorldState { players } = ev
                    && players
                        .iter()
                        .any(|p| p.id == 1 && (p.position[0] + 3.0).abs() < 0.01)
                    && players
                        .iter()
                        .any(|p| p.id == 0 && (p.position[0] - 4.0).abs() < 0.01)
                {
                    other_saw_client = true;
                }
            }
            if host_saw_client && other_saw_client {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(
            host_saw_client,
            "host should receive the UDP client's transform"
        );
        assert!(
            other_saw_client,
            "a second UDP client should see the first and the host"
        );

        // Leaving says Bye: the host drops the player at once.
        clients[0].disconnect();
        let mut left = false;
        for _ in 0..100 {
            if host
                .poll_events()
                .iter()
                .any(|ev| matches!(ev, NetEvent::PlayerLeft { id: 1 }))
            {
                left = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(left, "host should see the player leave");

        // The host ending the game tells the others.
        host.disconnect();
        let mut ended = false;
        for _ in 0..100 {
            if clients[1]
                .poll_events()
                .iter()
                .any(|ev| matches!(ev, NetEvent::Disconnected(_)))
            {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ended, "client should hear the host ended the game");
    }

    #[test]
    fn udp_hits_deliver_and_deduplicate() {
        let mut tracker = HitTracker::default();
        assert!(tracker.record(1, 1));
        assert!(!tracker.record(1, 1), "duplicate hit_id must be rejected");
        assert!(tracker.record(1, 2));
        assert!(tracker.record(2, 1));

        let mut host = NetworkState::default();
        host.start_host_with_region(None);
        let mut port = None;
        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::HostingStarted { udp_port, .. } = ev {
                    port = Some(udp_port);
                }
            }
            if port.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let port = port.expect("host did not start");

        let mut client = NetworkState::default();
        client.join_udp(port, "test");
        assert!(wait_for(&client, Duration::from_secs(5), |e| matches!(
            e,
            NetEvent::Connected { your_id: 1, .. }
        )));

        // Client (Orange, id 1) hits Host (Blue, id 0).
        let c_hit = HitEvent {
            hit_id: 1,
            attacker: 1,
            victim: 0,
            damage: 34.0,
            velocity: [0.0, 0.0, -30.0],
            point: [0.0, 1.2, 0.0],
            water: false,
        };
        client.send_hit(c_hit);
        // Duplicate send should be deduplicated by the host.
        client.send_hit(c_hit);

        let mut host_hits = 0usize;
        let deadline = Instant::now() + Duration::from_millis(400);
        while Instant::now() < deadline {
            for ev in host.poll_events() {
                if let NetEvent::HitReceived(h) = ev {
                    assert_eq!(h.attacker, 1);
                    assert_eq!(h.victim, 0);
                    assert!((h.damage - 34.0).abs() < 0.01);
                    host_hits += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(host_hits, 1, "host should receive the hit exactly once");

        // Host (Blue, id 0) hits Client (Orange, id 1).
        let h_hit = HitEvent {
            hit_id: 1,
            attacker: 0,
            victim: 1,
            damage: 45.0,
            velocity: [0.0, 0.0, 30.0],
            point: [1.0, 1.1, 2.0],
            water: false,
        };
        host.send_hit(h_hit);
        assert!(
            wait_for(&client, Duration::from_secs(3), |e| matches!(
                e,
                NetEvent::HitReceived(h) if h.attacker == 0 && h.victim == 1 && (h.damage - 45.0).abs() < 0.01
            )),
            "client should receive the host's hit"
        );

        client.disconnect();
        host.disconnect();
    }
}

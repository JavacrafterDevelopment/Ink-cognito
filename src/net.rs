use crate::protocol::{
    ClientMessage, GAME_PORT, PlayerId, PlayerSnapshot, ServerMessage, player_color, read_frame,
    spawn_position, write_frame,
};
use bevy::prelude::*;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tailcat::{Client, Region, Server, ServerConfig, TcpHandler};
use tokio::{
    io::DuplexStream,
    runtime::Runtime,
    sync::{Mutex as AsyncMutex, Notify, mpsc},
};

/// Whether the local player is hosting a Tailcat server or connected as a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRole {
    Host,
    Client,
}

/// Commands sent from the Bevy game loop to the background networking tasks.
#[derive(Debug)]
enum OutboundCommand {
    UpdateLocalTransform { position: [f32; 3], yaw: f32 },
}

/// Events produced by the background Tailcat networking tasks for Bevy to process.
#[derive(Debug, Clone)]
pub enum NetEvent {
    HostingStarted {
        tailcat_addr: String,
        local_player: PlayerSnapshot,
    },
    Connected {
        your_id: PlayerId,
        players: Vec<PlayerSnapshot>,
    },
    PlayerJoined(PlayerSnapshot),
    PlayerLeft {
        id: PlayerId,
    },
    WorldState {
        players: Vec<PlayerSnapshot>,
    },
    Error(String),
    Disconnected(String),
}

struct ActiveSession {
    role: NetRole,
    tx_out: mpsc::UnboundedSender<OutboundCommand>,
    rx_in: Mutex<mpsc::UnboundedReceiver<NetEvent>>,
    shutdown: Arc<Notify>,
}

/// Bevy resource that owns the Tokio runtime and active Tailcat multiplayer session.
#[derive(Resource)]
pub struct NetworkState {
    runtime: Runtime,
    session: Option<ActiveSession>,
    pub role: Option<NetRole>,
    pub local_player_id: Option<PlayerId>,
    pub tailcat_addr: Option<String>,
    pub status_text: String,
    pub error_text: Option<String>,
    pub is_connecting: bool,
    pub players: HashMap<PlayerId, PlayerSnapshot>,
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
            status_text: String::new(),
            error_text: None,
            is_connecting: false,
            players: HashMap::new(),
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
        self.status_text = "Starting Tailcat host...".to_string();

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
            shutdown,
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
            shutdown,
        });
    }

    /// Sends the local player's position and facing yaw to the network layer.
    pub fn send_local_transform(&self, position: [f32; 3], yaw: f32) {
        if let Some(session) = &self.session {
            let _ = session
                .tx_out
                .send(OutboundCommand::UpdateLocalTransform { position, yaw });
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
        if let Some(session) = self.session.take() {
            session.shutdown.notify_waiters();
            drop(session);
        }
        self.role = None;
        self.local_player_id = None;
        self.tailcat_addr = None;
        self.is_connecting = false;
        self.status_text.clear();
        self.players.clear();
    }

    pub fn active_role(&self) -> Option<NetRole> {
        self.session.as_ref().map(|s| s.role).or(self.role)
    }
}

impl Drop for NetworkState {
    fn drop(&mut self) {
        self.disconnect();
    }
}

struct HostSharedState {
    players: HashMap<PlayerId, PlayerSnapshot>,
    clients: HashMap<PlayerId, mpsc::UnboundedSender<ServerMessage>>,
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
    };

    let shared = Arc::new(AsyncMutex::new(HostSharedState {
        players: HashMap::from([(0, host_player)]),
        clients: HashMap::new(),
    }));
    let next_id = Arc::new(AtomicU32::new(1));

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

    let server = match Server::start(config).await {
        Ok(server) => server,
        Err(err) => {
            let _ = tx_in.send(NetEvent::Error(format!(
                "Failed to start Tailcat host: {err}"
            )));
            return;
        }
    };

    let tailcat_addr = server.tailcat_addr();
    info!("Tailcat host listening. Share this address to join:\n{tailcat_addr}");
    let _ = tx_in.send(NetEvent::HostingStarted {
        tailcat_addr,
        local_player: host_player,
    });

    let mut broadcast_tick = tokio::time::interval(Duration::from_millis(33));
    broadcast_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                break;
            }
            cmd = rx_out.recv() => {
                match cmd {
                    Some(OutboundCommand::UpdateLocalTransform { position, yaw }) => {
                        let mut guard = shared.lock().await;
                        if let Some(host) = guard.players.get_mut(&0) {
                            host.position = position;
                            host.yaw = yaw;
                        }
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

    server.close().await;
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
    };

    let (client_tx, mut client_rx) = mpsc::unbounded_channel::<ServerMessage>();

    let welcome = {
        let mut guard = shared.lock().await;
        guard.broadcast(&ServerMessage::PlayerJoined(snapshot));
        guard.players.insert(player_id, snapshot);
        guard.clients.insert(player_id, client_tx);
        ServerMessage::Welcome {
            your_id: player_id,
            players: guard.snapshot_list(),
        }
    };

    let _ = tx_in.send(NetEvent::PlayerJoined(snapshot));

    let (mut reader, mut writer) = tokio::io::split(stream);

    if write_frame(&mut writer, &welcome).await.is_err() {
        cleanup_host_client(player_id, &shared, &tx_in).await;
        return;
    }

    let writer_shutdown = shutdown.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_shutdown.notified() => break,
                msg = client_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if write_frame(&mut writer, &msg).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            frame = read_frame::<_, ClientMessage>(&mut reader) => {
                match frame {
                    Ok(Some(ClientMessage::UpdateTransform { position, yaw })) => {
                        let players = {
                            let mut guard = shared.lock().await;
                            if let Some(player) = guard.players.get_mut(&player_id) {
                                player.position = position;
                                player.yaw = yaw;
                            }
                            guard.snapshot_list()
                        };
                        let _ = tx_in.send(NetEvent::WorldState { players });
                    }
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }

    writer_task.abort();
    cleanup_host_client(player_id, &shared, &tx_in).await;
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

    let (mut reader, mut writer) = tokio::io::split(stream);

    // Wait for initial Welcome message from host.
    match read_frame::<_, ServerMessage>(&mut reader).await {
        Ok(Some(ServerMessage::Welcome { your_id, players })) => {
            let _ = tx_in.send(NetEvent::Connected { your_id, players });
        }
        Ok(Some(_)) => {
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
        Err(err) => {
            client.close().await;
            let _ = tx_in.send(NetEvent::Error(format!("Handshake failed: {err}")));
            return;
        }
    }

    let mut latest_transform: Option<([f32; 3], f32)> = None;
    let mut send_tick = tokio::time::interval(Duration::from_millis(33));
    send_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                break;
            }
            cmd = rx_out.recv() => {
                match cmd {
                    Some(OutboundCommand::UpdateLocalTransform { position, yaw }) => {
                        latest_transform = Some((position, yaw));
                    }
                    None => break,
                }
            }
            _ = send_tick.tick() => {
                if let Some((position, yaw)) = latest_transform.take()
                    && write_frame(&mut writer, &ClientMessage::UpdateTransform { position, yaw })
                        .await
                        .is_err()
                {
                    let _ = tx_in.send(NetEvent::Disconnected(
                        "Lost connection to host.".to_string(),
                    ));
                    break;
                }
            }
            frame = read_frame::<_, ServerMessage>(&mut reader) => {
                match frame {
                    Ok(Some(ServerMessage::PlayerJoined(player))) => {
                        let _ = tx_in.send(NetEvent::PlayerJoined(player));
                    }
                    Ok(Some(ServerMessage::PlayerLeft { id })) => {
                        let _ = tx_in.send(NetEvent::PlayerLeft { id });
                    }
                    Ok(Some(ServerMessage::WorldState { players })) => {
                        let _ = tx_in.send(NetEvent::WorldState { players });
                    }
                    Ok(Some(ServerMessage::Welcome { your_id, players })) => {
                        let _ = tx_in.send(NetEvent::Connected { your_id, players });
                    }
                    Ok(None) => {
                        let _ = tx_in.send(NetEvent::Disconnected(
                            "Host ended the game.".to_string(),
                        ));
                        break;
                    }
                    Err(err) => {
                        let _ = tx_in.send(NetEvent::Disconnected(format!(
                            "Connection error: {err}"
                        )));
                        break;
                    }
                }
            }
        }
    }

    client.close().await;
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
                if let NetEvent::HostingStarted { tailcat_addr, .. } = ev {
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
        client.join_game(addr);

        let mut client_id = None;
        for _ in 0..200 {
            for ev in client.poll_events() {
                if let NetEvent::Connected { your_id, players } = ev {
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
        host.send_local_transform([4.0, 1.0, -2.5], 1.25);
        client.send_local_transform([-3.0, 1.0, 5.5], -0.75);

        let mut host_saw_client = false;
        let mut client_saw_host = false;

        for _ in 0..100 {
            for ev in host.poll_events() {
                if let NetEvent::WorldState { players } = ev
                    && players.iter().any(|p| {
                        p.id == client_id && (p.position[0] + 3.0).abs() < 0.01
                    })
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

        client.disconnect();
        host.disconnect();
    }
}

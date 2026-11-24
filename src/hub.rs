//! Playing through a Portside hub: a game you host shows up for everyone in
//! your hub who has Ink-cognito open, and you can join theirs.
//!
//! Hosting always listens for UDP on 127.0.0.1 (see `net`); when Portside is
//! running and you're in a hub, that port is offered to the hub as a session.
//! Joining asks Portside for the session, which gives back a port on this PC
//! that leads to the host's game over Portside's UDP tunnel.

use crate::{
    AppState,
    net::{NetRole, NetworkState, Transport},
    portside::{Event, Portside, Session, Snapshot, Status},
};
use bevy::prelude::*;
use serde_json::json;
use std::collections::HashSet;

/// How long a "someone started a game" pop-up stays.
pub const TOAST_SECS: f32 = 12.0;

pub struct HubPlugin;

impl Plugin for HubPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Portside>()
            .init_resource::<HubPlay>()
            .add_systems(PreUpdate, drive_portside);
    }
}

#[derive(Resource)]
pub struct HubPlay {
    pub snapshot: Snapshot,
    /// Our session's sid while it's shown in the hub.
    hosted: Option<String>,
    host_pending: bool,
    /// The port hosting failed for, so it isn't asked again and again.
    failed_port: Option<u16>,
    pub host_error: Option<String>,
    announced_players: usize,
    joining: Option<String>,
    joined: Option<String>,
    pub joined_title: Option<String>,
    activity_sent: Option<Option<String>>,
    known: HashSet<String>,
    primed: bool,
    /// The newest game someone started, and how long it's still shown.
    pub toast: Option<(Session, f32)>,
    /// Tunnels to close in a moment, once the game's goodbye has gone through
    /// them: (seconds left, sid we hosted or session id we joined, hosted?).
    closing: Vec<(f32, String, bool)>,
}

/// How long a tunnel stays up after we leave, for the goodbye to get through.
const CLOSE_AFTER: f32 = 1.0;

impl Default for HubPlay {
    fn default() -> Self {
        Self {
            snapshot: Snapshot {
                status: Status::Looking,
                my_name: None,
                hub: None,
                sessions: Vec::new(),
                version: 0,
            },
            hosted: None,
            host_pending: false,
            failed_port: None,
            host_error: None,
            announced_players: 0,
            joining: None,
            joined: None,
            joined_title: None,
            activity_sent: None,
            known: HashSet::new(),
            primed: false,
            toast: None,
            closing: Vec::new(),
        }
    }
}

impl HubPlay {
    /// Games others in the hub are hosting (not ours).
    pub fn others_sessions(&self) -> impl Iterator<Item = &Session> {
        self.snapshot.sessions.iter().filter(|s| !s.mine)
    }

    /// Starts joining a session: Portside opens the tunnel, then we play over it.
    pub fn start_join(&mut self, session: &Session, net: &mut NetworkState, portside: &Portside) {
        if let Some(old) = self.joining.take() {
            portside.leave(old);
        }
        let label = format!("{} (hosted by {})", session.title, session.host_name);
        net.begin_portside_join(&label);
        self.joining = Some(session.id.clone());
        self.joined_title = Some(session.title.clone());
        self.toast = None;
        portside.join(session.id.clone(), label);
    }

    /// The person pressed Cancel while Portside was still opening the tunnel.
    pub fn cancel_join(&mut self, portside: &Portside) {
        if let Some(id) = self.joining.take() {
            portside.leave(id);
        }
    }
}

fn hosting_port(net: &NetworkState) -> Option<u16> {
    (net.active_role() == Some(NetRole::Host))
        .then_some(net.udp_port)
        .flatten()
}

fn game_title(snapshot: &Snapshot) -> String {
    match &snapshot.my_name {
        Some(name) if !name.is_empty() => format!("{name}'s plate"),
        _ => "Ink-cognito plate".to_string(),
    }
}

fn drive_portside(
    time: Res<Time>,
    portside: Res<Portside>,
    mut hub: ResMut<HubPlay>,
    mut net: ResMut<NetworkState>,
    state: Res<State<AppState>>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    let snap = portside.snapshot();
    let hosting = hosting_port(&net);

    for event in portside.poll_events() {
        match event {
            Event::Hosted { sid } => {
                hub.host_pending = false;
                if hosting.is_some() {
                    info!("Shown in the Portside hub as session {sid}");
                    hub.hosted = Some(sid);
                    hub.host_error = None;
                } else {
                    portside.stop(sid);
                }
            }
            Event::HostFailed(err) => {
                hub.host_pending = false;
                hub.failed_port = hosting;
                warn!("Portside couldn't show this game in the hub: {err}");
                hub.host_error = Some(err);
            }
            Event::Joined {
                session,
                port,
                title,
            } => {
                if hub.joining.as_deref() == Some(session.as_str())
                    && net.is_connecting
                    && net.transport == Some(Transport::Portside)
                {
                    hub.joining = None;
                    hub.joined = Some(session);
                    net.join_udp(port, &title);
                } else {
                    portside.leave(session);
                }
            }
            Event::JoinFailed { session, error } => {
                if hub.joining.as_deref() == Some(session.as_str()) {
                    hub.joining = None;
                    net.disconnect();
                    net.error_text = Some(format!("Couldn't join through Portside: {error}"));
                }
            }
            Event::SessionEnded { sid, id, error } => {
                if sid.is_some() && sid == hub.hosted {
                    hub.hosted = None;
                    hub.failed_port = hosting;
                    hub.host_error = Some(error);
                } else if id.is_some() && id == hub.joined {
                    hub.joined = None;
                    if net.transport == Some(Transport::Portside) {
                        net.disconnect();
                        net.error_text = Some(format!("The hub game ended: {error}"));
                        if *state.get() == AppState::InGame {
                            next_state.set(AppState::MainMenu);
                        }
                    }
                }
            }
            Event::Lost => {
                hub.hosted = None;
                hub.host_pending = false;
                hub.failed_port = None;
                hub.joined = None;
                hub.activity_sent = None;
                hub.primed = false;
                if hub.joining.take().is_some() {
                    net.disconnect();
                    net.error_text = Some("Portside closed before the game could join.".into());
                }
            }
        }
    }

    // Show the game we host in the hub, and keep its player count current.
    let players = net.players.len().max(1);
    match hosting {
        Some(port) => {
            if snap.can_host()
                && hub.hosted.is_none()
                && !hub.host_pending
                && hub.failed_port != Some(port)
            {
                portside.host(port, game_title(&snap), json!({ "players": players }));
                hub.host_pending = true;
                hub.announced_players = players;
            }
            if let Some(sid) = hub.hosted.clone()
                && players != hub.announced_players
            {
                portside.update(sid, json!({ "players": players }));
                hub.announced_players = players;
            }
        }
        None => {
            if let Some(sid) = hub.hosted.take() {
                hub.closing.push((CLOSE_AFTER, sid, true));
            }
            hub.failed_port = None;
            hub.host_error = None;
        }
    }

    // We left a game we joined through Portside: close its tunnel.
    if net.transport != Some(Transport::Portside) {
        if let Some(id) = hub.joined.take() {
            hub.closing.push((CLOSE_AFTER, id, false));
        }
        if hub.joining.is_some() && !net.is_connecting {
            hub.cancel_join(&portside);
        }
    }

    let dt = time.delta_secs();
    hub.closing.retain_mut(|(left, id, hosted)| {
        *left -= dt;
        if *left > 0.0 {
            return true;
        }
        if *hosted {
            portside.stop(std::mem::take(id));
        } else {
            portside.leave(std::mem::take(id));
        }
        false
    });

    // "Playing Ink-cognito · …" next to our name in the hub.
    if snap.status == Status::Ready {
        let activity = if net.is_connecting {
            "Joining a game"
        } else {
            match net.active_role() {
                Some(NetRole::Host) => "Hosting a plate",
                Some(NetRole::Client) => "On a plate",
                None => "In the menu",
            }
        };
        let want = Some(activity.to_string());
        if hub.activity_sent.as_ref() != Some(&want) {
            portside.set_activity(want.clone());
            hub.activity_sent = Some(want);
        }
    }

    // A game someone just started pops up.
    if let Some((_, left)) = &mut hub.toast {
        *left -= time.delta_secs();
        if *left <= 0.0 {
            hub.toast = None;
        }
    }
    if snap.version != hub.snapshot.version {
        let current: HashSet<String> = snap.sessions.iter().map(|s| s.id.clone()).collect();
        if hub.primed {
            let in_this_one = hub.joined.as_ref();
            if let Some(new) = snap
                .sessions
                .iter()
                .rfind(|s| !s.mine && !hub.known.contains(&s.id) && Some(&s.id) != in_this_one)
            {
                info!("{} started a game: {}", new.host_name, new.title);
                hub.toast = Some((new.clone(), TOAST_SECS));
            }
        } else if snap.status == Status::Ready {
            hub.primed = true;
        }
        if let Some((s, _)) = &hub.toast
            && !current.contains(&s.id)
        {
            hub.toast = None;
        }
        hub.known = current;
        hub.snapshot = snap;
    }
}

/// One line saying where Portside stands, for the menu.
pub fn status_line(hub: &HubPlay) -> String {
    let snap = &hub.snapshot;
    match &snap.status {
        Status::Looking => "Looking for Portside...".into(),
        Status::NotRunning => {
            "Portside isn't running. Start it and join a hub to play with your hub.".into()
        }
        Status::Asking => "Portside is asking to allow Ink-cognito: answer in Portside.".into(),
        Status::Denied(_) => "Not allowed in Portside. Press Retry to ask again.".into(),
        Status::Ready => match &snap.hub {
            None => "Portside is on. Join or start a hub to see games here.".into(),
            Some(h) if !h.games => format!(
                "In {}, but its host runs an older Portside (needs 0.16.0+).",
                h.name
            ),
            Some(h) => {
                let n = hub.others_sessions().count();
                match n {
                    0 => format!(
                        "In {}: no one's hosting yet. Create a game to show it here.",
                        h.name
                    ),
                    1 => format!("In {}: 1 game to join", h.name),
                    _ => format!("In {}: {n} games to join", h.name),
                }
            }
        },
    }
}

/// The in-game line about the hub.
pub fn hud_line(hub: &HubPlay, net: &NetworkState) -> Option<String> {
    let snap = &hub.snapshot;
    if net.transport == Some(Transport::Portside) {
        let hub_name = snap.hub.as_ref().map_or("your hub", |h| h.name.as_str());
        return Some(format!("Playing through Portside: {hub_name}"));
    }
    if net.active_role() != Some(NetRole::Host) {
        return None;
    }
    if let Some(err) = &hub.host_error {
        return Some(format!("Portside: {err}"));
    }
    Some(match (&snap.status, &snap.hub) {
        (Status::Ready, Some(h)) if hub.hosted.is_some() => {
            format!("Shown in your Portside hub: {}", h.name)
        }
        (Status::Ready, Some(h)) if !h.games => {
            "This hub's host needs Portside 0.16.0+ to show games".into()
        }
        (Status::Ready, Some(_)) => "Showing in your Portside hub...".into(),
        (Status::Ready, None) => "Join a Portside hub to show this game there".into(),
        (Status::Asking, _) => "Allow Ink-cognito in Portside to show it in your hub".into(),
        _ => "Start Portside to show this game in your hub".into(),
    })
}

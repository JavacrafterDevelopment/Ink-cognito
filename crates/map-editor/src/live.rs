//! Live editing: one person hosts the map they have open, friends join and
//! edit it with them, and everyone sees every change as it happens.
//!
//! Joining works the way the game's does: friends in your Portside hub see
//! the map you host and join it with a click (through Portside's game API),
//! or anyone can paste the host's TailCat address. The host's map is the real
//! one and only the host saves over its file; a guest's Save copy goes to
//! their Downloads folder (they can switch that to their own maps folder).
//!
//! How edits stay in step is in `live_sync.rs`; the connections are in
//! `live_net.rs`.

use crate::{
    files,
    live_net::{EDIT_PORT, LiveNet, Look, NetIn, NetOut, Person, Target, ToGuest, ToHost, VERSION},
    live_sync::{self, Op},
    state::{Editor, Field, Sel},
    ui::{self, Btn},
    view::FlyCam,
};
use bevy::{picking::prelude::*, prelude::*};
use ink_map::MapFile;
use ink_portside::{AppInfo, Event as PortsideEvent, Portside, Status};
use serde_json::json;
use std::collections::HashMap;

/// The map editor is a program of its own to Portside: it asks once to allow
/// it, and its sessions only reach other map editors.
const EDITOR_APP: AppInfo = AppInfo {
    id: "com.javacrafter.ink-cognito.map-editor",
    name: "Ink-cognito Map Editor",
    token_file: "map-editor-portside-token.json",
    token_env: "INK_MAP_EDITOR_PORTSIDE_TOKEN",
};

/// How often changes go out (seconds).
const SEND_EVERY: f32 = 1.0 / 15.0;
/// How often cameras go out (seconds).
const LOOK_EVERY: f32 = 0.2;

pub struct LivePlugin;

impl Plugin for LivePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Live>()
            .add_systems(Update, (update, draw_people, rebuild_window).chain())
            .add_observer(on_click);
    }
}

/// What the top bar's Live button says ("" when no session is on).
pub fn badge(ed: &Editor) -> String {
    ed.live.badge.clone()
}

#[derive(Resource)]
pub struct Live {
    /// Started the first time the live window opens (so Portside only asks
    /// about the editor once someone wants live editing).
    portside: Option<Portside>,
    net: Option<LiveNet>,
    mode: Mode,
    /// A guest's copies go to Downloads (else the game's maps folder).
    to_downloads: bool,
    /// A join waiting for a second click because of unsaved changes.
    join_armed: bool,
    /// Bumped when the window's contents change.
    version: u64,
    last_send: f32,
    last_look: f32,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            portside: None,
            net: None,
            mode: Mode::Off,
            to_downloads: true,
            join_armed: false,
            version: 0,
            last_send: 0.0,
            last_look: 0.0,
        }
    }
}

enum Mode {
    Off,
    Hosting(Host),
    Guest(Guest),
}

struct Host {
    /// The 127.0.0.1 TCP port Portside sessions lead to.
    port: Option<u16>,
    tailcat: Option<String>,
    tailcat_error: Option<String>,
    /// Shown in the Portside hub as this session.
    sid: Option<String>,
    portside_asked: bool,
    portside_error: Option<String>,
    /// The hub we were in when that went wrong (moving hubs tries again).
    seen_hub: Option<String>,
    /// What the hub was last told (players, map name).
    portside_info: Option<serde_json::Value>,
    people: Vec<Person>,
    /// Each guest's last change applied, by id.
    acks: HashMap<u32, u64>,
    /// The map as the guests have it.
    base: MapFile,
    looks: HashMap<u32, Look>,
}

enum Via {
    Tailcat,
    /// Through a Portside session: its id, and whether its tunnel is open yet.
    Portside { session: String, open: bool },
}

struct Guest {
    via: Via,
    host_name: String,
    /// Set by the host's welcome.
    you: Option<u32>,
    /// The map as the host last had it (with our changes that went out).
    base: Option<MapFile>,
    /// Our last batch of changes sent, and the last the host has applied.
    seq: u64,
    acked: u64,
    resync_asked: bool,
    people: Vec<Person>,
    looks: Vec<Look>,
}

impl Live {
    fn portside(&mut self) -> &Portside {
        self.portside.get_or_insert_with(|| Portside::new(EDITOR_APP))
    }

    fn net(&mut self) -> &mut LiveNet {
        self.net.get_or_insert_with(LiveNet::default)
    }

    fn my_name(&self) -> String {
        self.portside
            .as_ref()
            .and_then(|p| p.snapshot().my_name)
            .or_else(|| std::env::var("USERNAME").ok())
            .or_else(|| std::env::var("USER").ok())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Someone".into())
    }

    fn start_hosting(&mut self, ed: &mut Editor) {
        self.end(ed, "");
        let name = self.my_name();
        self.net().host();
        self.mode = Mode::Hosting(Host {
            port: None,
            tailcat: None,
            tailcat_error: None,
            sid: None,
            portside_asked: false,
            portside_error: None,
            seen_hub: None,
            portside_info: None,
            people: vec![Person { id: 0, name }],
            acks: HashMap::new(),
            base: ed.map.clone(),
            looks: HashMap::new(),
        });
        ed.status = "Hosting live editing. Friends in your Portside hub can join, or share the TailCat address.".into();
        self.version += 1;
    }

    fn start_guest(&mut self, ed: &mut Editor, via: Via, host_name: String) {
        self.end(ed, "");
        if matches!(via, Via::Tailcat) {
            let addr = ed.live.join_addr.trim().to_string();
            let name = self.my_name();
            self.net().join(Target::Tailcat(addr), name);
        }
        self.mode = Mode::Guest(Guest {
            via,
            host_name,
            you: None,
            base: None,
            seq: 0,
            acked: 0,
            resync_asked: false,
            people: Vec::new(),
            looks: Vec::new(),
        });
        ed.status = "Joining...".into();
        self.version += 1;
    }

    /// Ends whatever session is on. `why` goes in the status line (if any).
    fn end(&mut self, ed: &mut Editor, why: &str) {
        match std::mem::replace(&mut self.mode, Mode::Off) {
            Mode::Off => return,
            Mode::Hosting(h) => {
                if let (Some(sid), Some(p)) = (h.sid, &self.portside) {
                    p.stop(sid);
                }
            }
            Mode::Guest(g) => {
                if let (Via::Portside { session, .. }, Some(p)) = (g.via, &self.portside) {
                    p.leave(session);
                }
                ed.live.guest = false;
            }
        }
        if let Some(net) = &mut self.net {
            net.stop();
        }
        if !why.is_empty() {
            ed.status = why.to_string();
        }
        self.version += 1;
    }
}

// --- Keeping in step -------------------------------------------------------------

fn send(net: &Option<LiveNet>, msg: NetOut) {
    if let Some(net) = net {
        net.send(msg);
    }
}

/// Applies changes that came from someone else to what's on screen and to
/// undo history, keeping the selection on the same thing.
fn apply_remote(ed: &mut Editor, ops: &[Op]) {
    if ops.is_empty() {
        return;
    }
    let mut structure = false;
    for op in ops {
        let done = live_sync::apply(&mut ed.map, op);
        structure |= done.structure;
        if let Some(Sel::Object(i)) = ed.sel {
            if let Some(r) = done.removed {
                ed.sel = if r == i { None } else if r < i { Some(Sel::Object(i - 1)) } else { ed.sel };
            }
            if let Some(n) = done.inserted
                && n <= i
            {
                ed.sel = Some(Sel::Object(i + 1));
            }
        }
        if matches!(op, Op::Full(_)) {
            ed.clear_history();
        } else {
            for m in ed.history_mut() {
                live_sync::apply(m, op);
            }
        }
    }
    ed.sel = ed.sel.filter(|s| ed.exists(*s));
    if structure {
        ed.structure += 1;
    }
    ed.dirty = true;
}

fn people_names(people: &[Person]) -> String {
    people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// Where my camera is and what I have selected.
fn my_look(ed: &Editor, cam: Option<&Transform>, id: u32) -> Option<Look> {
    let cam = cam?;
    Some(Look {
        id,
        eye: cam.translation.to_array(),
        dir: cam.forward().as_vec3().to_array(),
        sel: ed.sel_pos().map(|p| p.to_array()),
    })
}

fn update(mut live: ResMut<Live>, mut editor: ResMut<Editor>, time: Res<Time>, cams: Query<&Transform, With<FlyCam>>) {
    let now = time.elapsed_secs();
    let live = &mut *live;

    if editor.live.window && live.portside.is_none() {
        live.portside();
    }

    // Portside: hub sessions hosted, joined and ended.
    let events = live.portside.as_ref().map(|p| p.poll_events()).unwrap_or_default();
    let mut ended: Option<String> = None;
    for ev in events {
        live.version += 1;
        match (&mut live.mode, ev) {
            (Mode::Hosting(h), PortsideEvent::Hosted { sid }) => {
                h.sid = Some(sid);
                h.portside_error = None;
            }
            (Mode::Hosting(h), PortsideEvent::HostFailed(e)) => {
                h.portside_asked = false;
                h.portside_error = Some(e);
            }
            (Mode::Hosting(h), PortsideEvent::SessionEnded { sid: Some(sid), error, .. }) if h.sid.as_deref() == Some(sid.as_str()) => {
                h.sid = None;
                h.portside_asked = false;
                h.portside_info = None;
                h.portside_error = Some(error);
            }
            (Mode::Hosting(h), PortsideEvent::Lost) => {
                h.sid = None;
                h.portside_asked = false;
                h.portside_info = None;
            }
            (Mode::Guest(g), PortsideEvent::Joined { session, port, .. }) => {
                if let Via::Portside { session: want, open } = &mut g.via
                    && *want == session
                    && !*open
                {
                    *open = true;
                    let name = live.portside.as_ref().and_then(|p| p.snapshot().my_name).unwrap_or_else(|| "Someone".into());
                    live.net.get_or_insert_with(LiveNet::default).join(Target::Local(port), name);
                }
            }
            (Mode::Guest(g), PortsideEvent::JoinFailed { session, error }) => {
                if matches!(&g.via, Via::Portside { session: want, .. } if *want == session) {
                    ended = Some(format!("Couldn't join: {error}"));
                }
            }
            (Mode::Guest(g), PortsideEvent::SessionEnded { id: Some(id), error, .. }) => {
                if matches!(&g.via, Via::Portside { session: want, .. } if *want == id) {
                    ended = Some(format!("Live editing ended: {error}"));
                }
            }
            (Mode::Guest(g), PortsideEvent::Lost) => {
                if matches!(g.via, Via::Portside { .. }) {
                    ended = Some("Live editing ended: lost the connection to Portside.".into());
                }
            }
            _ => {}
        }
    }
    if let Some(why) = ended.take() {
        live.end(&mut editor, &why);
    }

    // Hosting: show the session in the Portside hub when we can, and keep
    // its player count and map name current.
    if let Mode::Hosting(h) = &mut live.mode
        && let (Some(port), Some(portside)) = (h.port, &live.portside)
    {
        let snap = portside.snapshot();
        let hub = snap.hub.as_ref().map(|h| h.id.clone());
        if h.seen_hub != hub {
            h.seen_hub = hub;
            h.portside_error = None;
        }
        let info = json!({"map": editor.map.name, "players": h.people.len(), "port": EDIT_PORT});
        if h.sid.is_none() && !h.portside_asked && h.portside_error.is_none() && snap.can_host() {
            let title = if editor.map.name.trim().is_empty() { "A map".to_string() } else { editor.map.name.clone() };
            portside.host(port, title, info.clone());
            h.portside_asked = true;
            h.portside_info = Some(info);
            live.version += 1;
        } else if let Some(sid) = &h.sid
            && h.portside_info.as_ref() != Some(&info)
        {
            portside.update(sid.clone(), info.clone());
            h.portside_info = Some(info);
        }
    }

    // The connections.
    let incoming = live.net.as_ref().map(|n| n.poll()).unwrap_or_default();
    for msg in incoming {
        // Changes and cameras don't change what the window shows.
        let busywork = matches!(
            msg,
            NetIn::FromGuest { msg: ToHost::Ops { .. } | ToHost::Look(_), .. }
                | NetIn::FromHost(ToGuest::Ops { .. } | ToGuest::Looks(_) | ToGuest::Ping)
        );
        if !busywork {
            live.version += 1;
        }
        if let NetIn::Ended(why) = &msg {
            let why = if why.is_empty() { "Live editing ended.".to_string() } else { why.clone() };
            live.end(&mut editor, &why);
            continue;
        }
        let result = match &mut live.mode {
            Mode::Hosting(h) => {
                host_message(h, &live.net, &mut editor, msg);
                Ok(())
            }
            Mode::Guest(g) => guest_message(g, &live.net, &mut editor, live.to_downloads, msg),
            Mode::Off => Ok(()),
        };
        if let Err(why) = result {
            live.end(&mut editor, &why);
        }
    }

    // Our own changes go out a few times a second.
    if now - live.last_send >= SEND_EVERY {
        live.last_send = now;
        match &mut live.mode {
            Mode::Hosting(h) => flush_host(h, &live.net, &editor),
            Mode::Guest(g) => {
                if let Some(base) = &mut g.base {
                    let ops = live_sync::diff(base, &editor.map);
                    if !ops.is_empty() {
                        *base = editor.map.clone();
                        g.seq += 1;
                        send(&live.net, NetOut::Host(ToHost::Ops { seq: g.seq, ops }));
                    }
                }
            }
            Mode::Off => {}
        }
    }

    // Where everyone is looking.
    if now - live.last_look >= LOOK_EVERY {
        live.last_look = now;
        let cam = cams.single().ok();
        match &mut live.mode {
            Mode::Hosting(h) => {
                if let Some(l) = my_look(&editor, cam, 0) {
                    h.looks.insert(0, l);
                }
                if h.people.len() > 1 {
                    let looks = h.looks.values().cloned().collect();
                    send(&live.net, NetOut::All { except: None, msg: ToGuest::Looks(looks) });
                }
            }
            Mode::Guest(g) => {
                if let Some(you) = g.you
                    && let Some(l) = my_look(&editor, cam, you)
                {
                    send(&live.net, NetOut::Host(ToHost::Look(l)));
                }
            }
            Mode::Off => {}
        }
    }

    // The top bar's button.
    let badge = match &live.mode {
        Mode::Off => String::new(),
        Mode::Hosting(h) => format!("Live: hosting, {} editing", h.people.len()),
        Mode::Guest(g) if g.you.is_none() => "Live: joining...".into(),
        Mode::Guest(g) => format!("Live: {} editing", g.people.len()),
    };
    if editor.live.badge != badge {
        editor.live.badge = badge;
    }
}

/// Hosting: sends the host's own changes since last time to every guest.
fn flush_host(h: &mut Host, net: &Option<LiveNet>, ed: &Editor) {
    let ops = live_sync::diff(&h.base, &ed.map);
    if ops.is_empty() {
        return;
    }
    h.base = ed.map.clone();
    let hash = live_sync::hash(&h.base);
    if let Some(net) = net {
        for (&id, &ack) in &h.acks {
            net.send(NetOut::To(id, ToGuest::Ops { ops: ops.clone(), hash, ack }));
        }
    }
}

fn host_message(h: &mut Host, net: &Option<LiveNet>, ed: &mut Editor, msg: NetIn) {
    match msg {
        NetIn::Listening { port } => h.port = Some(port),
        NetIn::TailcatReady(addr) => h.tailcat = Some(addr),
        NetIn::TailcatFailed(e) => h.tailcat_error = Some(e),
        NetIn::GuestJoined { id, name, version } => {
            if version != VERSION {
                send(net, NetOut::To(
                    id,
                    ToGuest::Bye { why: "Your map editor is a different version from the host's. Update Ink-cognito on both PCs.".into() },
                ));
                return;
            }
            // Everyone else gets what's changed so far, so the welcome map is the same as theirs.
            flush_host(h, net, ed);
            let hash = live_sync::hash(&h.base);
            send(net, NetOut::To(
                id,
                ToGuest::Welcome { you: id, file_name: ed.file_name.clone(), map: Box::new(h.base.clone()), hash },
            ));
            h.acks.insert(id, 0);
            ed.status = format!("{name} joined.");
            h.people.push(Person { id, name });
            send(net, NetOut::All { except: None, msg: ToGuest::People(h.people.clone()) });
        }
        NetIn::GuestLeft { id } => {
            h.acks.remove(&id);
            h.looks.remove(&id);
            if let Some(i) = h.people.iter().position(|p| p.id == id) {
                let p = h.people.remove(i);
                ed.status = format!("{} left.", p.name);
            }
            send(net, NetOut::All { except: None, msg: ToGuest::People(h.people.clone()) });
        }
        NetIn::FromGuest { id, msg: ToHost::Ops { seq, ops } } => {
            if !h.acks.contains_key(&id) {
                return;
            }
            // Our own changes first, so theirs land on the map the guests have.
            flush_host(h, net, ed);
            let before = ed.map.clone();
            apply_remote(ed, &ops);
            h.acks.insert(id, seq);
            let done = live_sync::diff(&before, &ed.map);
            h.base = ed.map.clone();
            let hash = live_sync::hash(&h.base);
            for (&other, &ack) in &h.acks {
                let ops = if other == id { Vec::new() } else { done.clone() };
                if other == id || !ops.is_empty() {
                    send(net, NetOut::To(other, ToGuest::Ops { ops, hash, ack }));
                }
            }
        }
        NetIn::FromGuest { id, msg: ToHost::Resync } => {
            flush_host(h, net, ed);
            let hash = live_sync::hash(&h.base);
            let ack = h.acks.get(&id).copied().unwrap_or(0);
            send(net, NetOut::To(id, ToGuest::Ops { ops: vec![Op::Full(Box::new(h.base.clone()))], hash, ack }));
        }
        NetIn::FromGuest { id, msg: ToHost::Look(mut l) } => {
            if h.acks.contains_key(&id) {
                l.id = id;
                h.looks.insert(id, l);
            }
        }
        NetIn::FromGuest { .. } | NetIn::FromHost(_) | NetIn::Ended(_) => {}
    }
}

/// `Err`: the session is over, and why.
fn guest_message(g: &mut Guest, net: &Option<LiveNet>, ed: &mut Editor, to_downloads: bool, msg: NetIn) -> Result<(), String> {
    let NetIn::FromHost(msg) = msg else {
        return Ok(());
    };
    match msg {
        ToGuest::Welcome { you, file_name, map, hash: _ } => {
            g.you = Some(you);
            ed.map = (*map).clone();
            ed.file_name = file_name;
            ed.path = None;
            ed.sel = None;
            ed.dirty = false;
            ed.clear_history();
            ed.structure += 1;
            ed.live.guest = true;
            ed.live.save_dir = if to_downloads { files::downloads_dir() } else { None };
            ed.status = format!(
                "Editing {}'s map live. Your changes show up for everyone; Save copy saves to {}.",
                g.host_name,
                save_place(ed)
            );
            g.base = Some(*map);
        }
        ToGuest::Ops { ops, hash, ack } => {
            let Some(base) = &mut g.base else {
                return Ok(());
            };
            let full = ops.iter().any(|o| matches!(o, Op::Full(_)));
            for op in &ops {
                live_sync::apply(base, op);
            }
            apply_remote(ed, &ops);
            if full {
                g.resync_asked = false;
            }
            g.acked = ack;
            // Everything we sent is in: we should have the host's map now.
            if g.acked == g.seq && !g.resync_asked && live_sync::hash(base) != hash {
                g.resync_asked = true;
                if let Some(net) = net {
                    net.send(NetOut::Host(ToHost::Resync));
                }
            }
        }
        ToGuest::People(people) => {
            if let Some(h) = people.iter().find(|p| p.id == 0) {
                g.host_name = h.name.clone();
            }
            g.people = people;
        }
        ToGuest::Looks(looks) => g.looks = looks,
        ToGuest::Ping => {}
        ToGuest::Bye { why } => return Err(why),
    }
    Ok(())
}

/// Where a guest's Save copy goes, for the status line.
fn save_place(ed: &Editor) -> String {
    match &ed.live.save_dir {
        Some(d) => d.display().to_string(),
        None => "your maps folder".into(),
    }
}

// --- Other people in the view --------------------------------------------------

fn person_color(id: u32) -> Color {
    const PALETTE: [[f32; 3]; 8] = [
        [0.24, 0.62, 0.96],
        [0.95, 0.36, 0.36],
        [0.25, 0.80, 0.52],
        [0.96, 0.72, 0.22],
        [0.68, 0.44, 0.96],
        [0.18, 0.82, 0.86],
        [0.95, 0.46, 0.74],
        [0.64, 0.86, 0.26],
    ];
    let [r, g, b] = PALETTE[id as usize % PALETTE.len()];
    Color::srgb(r, g, b)
}

/// Each other person: a small ring where their camera is, a line where it
/// looks, and a ring on the ground at what they have selected.
fn draw_people(live: Res<Live>, mut gizmos: Gizmos) {
    let (looks, me): (Vec<&Look>, Option<u32>) = match &live.mode {
        Mode::Hosting(h) => (h.looks.values().collect(), Some(0)),
        Mode::Guest(g) => (g.looks.iter().collect(), g.you),
        Mode::Off => return,
    };
    for l in looks {
        if Some(l.id) == me {
            continue;
        }
        let color = person_color(l.id);
        let eye = Vec3::from_array(l.eye);
        let dir = Vec3::from_array(l.dir).normalize_or(Vec3::NEG_Z);
        circle(&mut gizmos, eye, Vec3::Y, 0.5, color);
        circle(&mut gizmos, eye, dir.any_orthonormal_vector(), 0.5, color);
        gizmos.line(eye, eye + dir * 2.5, color);
        if let Some(sel) = l.sel {
            let at = Vec3::from_array(sel) + Vec3::Y * 0.03;
            circle(&mut gizmos, at, Vec3::Y, 1.1, color);
            circle(&mut gizmos, at, Vec3::Y, 1.2, color);
        }
    }
}

/// A circle of radius `r` around `at`, facing `normal`.
fn circle(gizmos: &mut Gizmos, at: Vec3, normal: Vec3, r: f32, color: Color) {
    let rot = Quat::from_rotation_arc(Vec3::Y, normal.normalize_or(Vec3::Y));
    let n = 24;
    let point = |i: usize| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        at + rot * Vec3::new(a.cos() * r, 0.0, a.sin() * r)
    };
    for i in 0..n {
        gizmos.line(point(i), point(i + 1), color);
    }
}

// --- The window ------------------------------------------------------------------

#[derive(Component, Clone, Debug)]
enum LiveAction {
    Close,
    Host,
    Stop,
    /// A Portside session: its id, title and host.
    JoinSession(String, String, String),
    JoinAddress,
    Paste,
    CopyAddress,
    SaveTo,
    RetryPortside,
}

#[derive(Component)]
struct LiveRoot;

fn on_click(click: On<PointerClick>, actions: Query<&LiveAction>, mut live: ResMut<Live>, mut editor: ResMut<Editor>) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };
    let ed = &mut *editor;
    ed.focus = None;
    live.version += 1;
    let joining = matches!(action, LiveAction::JoinSession(..) | LiveAction::JoinAddress);
    if joining && ed.dirty && !live.join_armed {
        live.join_armed = true;
        ed.status = "Joining replaces the map you have open, which has unsaved changes. Click Join again to go ahead, or Save first.".into();
        return;
    }
    live.join_armed = false;
    match action.clone() {
        LiveAction::Close => ed.live.window = false,
        LiveAction::Host => live.start_hosting(ed),
        LiveAction::Stop => {
            let why = if ed.live.guest { "Left live editing. The map stays open here." } else { "Stopped live editing." };
            live.end(ed, why);
        }
        LiveAction::JoinSession(session, title, host) => {
            live.start_guest(ed, Via::Portside { session: session.clone(), open: false }, host.clone());
            live.portside().join(session, title);
            ed.status = format!("Opening Portside's tunnel to {host}...");
        }
        LiveAction::JoinAddress => {
            if ed.live.join_addr.trim().is_empty() {
                ed.status = "Paste the host's TailCat address first.".into();
            } else {
                live.start_guest(ed, Via::Tailcat, "the host".into());
                ed.status = "Connecting over TailCat...".into();
            }
        }
        LiveAction::Paste => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => ed.live.join_addr = text.trim().chars().take(400).collect(),
            Err(e) => ed.status = format!("Couldn't paste: {e}"),
        },
        LiveAction::CopyAddress => {
            if let Mode::Hosting(Host { tailcat: Some(addr), .. }) = &live.mode {
                ed.status = match arboard::Clipboard::new().and_then(|mut c| c.set_text(addr.clone())) {
                    Ok(()) => "Copied the TailCat address. Send it to whoever you want to edit with.".into(),
                    Err(e) => format!("Couldn't copy: {e}"),
                };
            }
        }
        LiveAction::SaveTo => {
            live.to_downloads = !live.to_downloads;
            if ed.live.guest {
                ed.live.save_dir = if live.to_downloads { files::downloads_dir() } else { None };
                ed.path = None;
                ed.status = format!("Save copy now saves to {}.", save_place(ed));
            }
        }
        LiveAction::RetryPortside => {
            live.portside().retry();
            if let Mode::Hosting(h) = &mut live.mode {
                h.portside_error = None;
                h.portside_asked = false;
            }
        }
    }
}

fn button(p: &mut ChildSpawnerCommands, text: impl Into<String>, action: LiveAction, on: bool, grow: bool) {
    let (normal, hover) = if on { (ui::BTN_ON, ui::BTN_ON_HOVER) } else { (ui::BTN, ui::BTN_HOVER) };
    p.spawn((
        Node {
            padding: UiRect::axes(Val::Px(9.0), Val::Px(5.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_grow: if grow { 1.0 } else { 0.0 },
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(normal),
        Btn { normal, hover },
        action,
    ))
    .with_children(|b| ui::label(b, text, 13.0, ui::TEXT));
}

fn portside_line(live: &Live) -> (String, bool) {
    let Some(p) = &live.portside else {
        return ("Looking for Portside...".into(), false);
    };
    let s = p.snapshot();
    match &s.status {
        Status::Looking => ("Looking for Portside...".into(), false),
        Status::NotRunning => ("Portside isn't running. Start it to edit with friends in your hub, or use a TailCat address.".into(), false),
        Status::Asking => ("Portside is asking whether to allow the map editor. Click Allow there.".into(), false),
        Status::Denied(why) => (format!("Portside: {why}"), true),
        Status::Ready => match &s.hub {
            None => ("Portside is running, but you're not in a hub.".into(), false),
            Some(h) if !h.games => (format!("{}'s host needs a newer Portside for sessions.", h.name), false),
            Some(h) => (format!("Portside: in {} ({} people).", h.name, h.members), false),
        },
    }
}

fn window_signature(live: &Live, ed: &Editor, blink: bool) -> String {
    let portside = live.portside.as_ref().map(|p| p.snapshot().version);
    format!(
        "{}|{}|{:?}|{}|{}|{:?}|{}|{}|{}",
        ed.live.window,
        live.version,
        portside,
        ed.live.join_addr,
        blink,
        ed.focus,
        ed.dirty,
        ed.live.guest,
        live.to_downloads
    )
}

fn rebuild_window(
    mut commands: Commands,
    live: Res<Live>,
    editor: Res<Editor>,
    roots: Query<Entity, With<LiveRoot>>,
    time: Res<Time>,
    mut last: Local<String>,
) {
    let blink = editor.focus == Some(Field::JoinAddress) && ((time.elapsed_secs() * 2.0) as u32).is_multiple_of(2);
    let sig = window_signature(&live, &editor, blink);
    if *last == sig {
        return;
    }
    *last = sig;
    for e in &roots {
        commands.entity(e).despawn();
    }
    if !editor.live.window {
        return;
    }
    let ed = &*editor;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            GlobalZIndex(10),
            LiveRoot,
        ))
        .with_children(|bg| {
            bg.spawn((
                Node {
                    width: Val::Px(500.0),
                    max_height: Val::Percent(85.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(14.0)),
                    row_gap: Val::Px(6.0),
                    overflow: Overflow::clip(),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    ..default()
                },
                BackgroundColor(ui::PANEL),
            ))
            .with_children(|p| window_body(p, &live, ed, blink));
        });
}

fn window_body(p: &mut ChildSpawnerCommands, live: &Live, ed: &Editor, blink: bool) {
    ui::label(p, "LIVE EDITING", 15.0, ui::ACCENT);
    let (line, retry) = portside_line(live);
    ui::row(p, |r| {
        ui::label(r, line, 12.0, ui::MUTED);
        if retry {
            button(r, "Try again", LiveAction::RetryPortside, false, false);
        }
    });
    match &live.mode {
        Mode::Off => {
            ui::label(p, "Edit this map together: friends see every change you make, and you see theirs.", 12.0, ui::TEXT);
            ui::row(p, |r| button(r, "Host this map", LiveAction::Host, false, true));
            ui::label(
                p,
                "Friends in your Portside hub see it in their map editor's Live window. You also get a TailCat address to send anyone.",
                11.0,
                ui::MUTED,
            );

            ui::heading(p, "Join a friend");
            let sessions: Vec<_> = live
                .portside
                .as_ref()
                .map(|ps| ps.snapshot().sessions.into_iter().filter(|s| !s.mine).collect())
                .unwrap_or_default();
            if sessions.is_empty() {
                ui::label(p, "Nobody in your hub is hosting a map right now.", 12.0, ui::MUTED);
            }
            for s in sessions {
                let text = match s.players {
                    Some(n) => format!("{}  (hosted by {}, {n} editing)", s.title, s.host_name),
                    None => format!("{}  (hosted by {})", s.title, s.host_name),
                };
                ui::row(p, |r| button(r, text, LiveAction::JoinSession(s.id.clone(), s.title.clone(), s.host_name.clone()), false, true));
            }
            ui::label(p, "Or paste the host's TailCat address:", 12.0, ui::MUTED);
            ui::text_field(p, ed, Field::JoinAddress, &ed.live.join_addr, blink, Val::Percent(100.0));
            ui::row(p, |r| {
                button(r, "Paste", LiveAction::Paste, false, false);
                button(r, "Join", LiveAction::JoinAddress, false, false);
            });
            if ed.dirty {
                ui::label(p, "Joining replaces the map you have open here. Save it first if you want to keep it.", 11.0, ui::ACCENT);
            }
        }
        Mode::Hosting(h) => {
            ui::label(p, format!("Hosting \"{}\". {} editing: {}", ed.map.name, h.people.len(), people_names(&h.people)), 13.0, ui::TEXT);
            let hub = match (&h.sid, &h.portside_error, h.port) {
                (Some(_), _, _) => "Shown in your Portside hub: friends there can join from their Live window.".to_string(),
                (None, Some(e), _) => format!("Not in the Portside hub: {e}"),
                (None, None, None) => "Starting...".to_string(),
                (None, None, Some(_)) => "Not in a Portside hub yet: it shows up there once you're in one.".to_string(),
            };
            ui::label(p, hub, 12.0, ui::MUTED);
            ui::heading(p, "TailCat address");
            match (&h.tailcat, &h.tailcat_error) {
                (Some(addr), _) => {
                    ui::label(p, addr.clone(), 11.0, ui::TEXT);
                    ui::row(p, |r| button(r, "Copy address", LiveAction::CopyAddress, false, false));
                }
                (None, Some(e)) => ui::label(p, e.clone(), 12.0, ui::ACCENT),
                (None, None) => ui::label(p, "Getting an address...", 12.0, ui::MUTED),
            }
            ui::label(p, "Only you save over this map's file. Guests can save copies of their own.", 11.0, ui::MUTED);
            ui::row(p, |r| button(r, "Stop hosting", LiveAction::Stop, false, true));
        }
        Mode::Guest(g) => {
            if g.you.is_none() {
                ui::label(p, ed.status.clone(), 13.0, ui::TEXT);
            } else {
                ui::label(p, format!("Editing {}'s map live. {} editing: {}", g.host_name, g.people.len(), people_names(&g.people)), 13.0, ui::TEXT);
                ui::label(p, format!("Save copy saves to {}.", save_place(ed)), 12.0, ui::MUTED);
            }
            ui::row(p, |r| {
                let to = if live.to_downloads { "Save copies to: Downloads" } else { "Save copies to: my maps folder" };
                button(r, to, LiveAction::SaveTo, false, true);
            });
            ui::row(p, |r| button(r, if g.you.is_none() { "Cancel" } else { "Leave" }, LiveAction::Stop, false, true));
        }
    }
    p.spawn((Node { height: Val::Px(4.0), ..default() }, Pickable::IGNORE));
    ui::row(p, |r| button(r, "Close", LiveAction::Close, false, false));
}

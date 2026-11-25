use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Virtual TCP port used inside the Tailcat tunnel for game traffic.
pub const GAME_PORT: u16 = 7777;

/// A player or host heard nothing from for this long is gone (crashed,
/// Alt+F4'd, lost its network). Pings go every couple of seconds.
pub const SILENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Upper bound on a single serialized network frame to protect against malformed packets.
const MAX_FRAME_BYTES: usize = 64 * 1024;

pub type PlayerId = u32;

/// Network-synced state for a single player in the 3D world.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub position: [f32; 3],
    pub yaw: f32,
    pub color: [f32; 3],
    #[serde(default)]
    pub state: PlayerState,
}

/// Everything about a player besides where they are: health, what they carry,
/// and counters others watch to see shots and eliminations happen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerState {
    /// Where they look up (+) or down (-), radians.
    pub pitch: f32,
    /// 0..=100. At 0 the player is eliminated until they respawn.
    pub health: f32,
    /// The packed loadout (see `loadout::Loadout::pack`).
    pub loadout: u32,
    /// Counts every shot fired: others fire one ball per step it goes up.
    pub shots: u32,
    /// Counts footsteps taken: others hear one step, where this player
    /// stands, each time it goes up.
    pub steps: u32,
    /// Counts eliminations of this player: others show a ragdoll per step.
    pub deaths: u32,
    /// Who made the last elimination.
    pub killer: PlayerId,
    /// What the body is doing (`FLAG_*`), for the animations others see.
    pub flags: u8,
    /// Ranked points (shown next to the name in ranked matches).
    pub rank_points: u16,
    /// Counts paint bombs thrown: others throw one from where this player
    /// looks each time it goes up (wraps).
    pub bombs: u8,
    /// Counts flashbangs thrown, the same way.
    pub flashes: u8,
}

/// `PlayerState::flags`: crouching.
pub const FLAG_CROUCH: u8 = 1;
/// `PlayerState::flags`: in the air (jumping or falling).
pub const FLAG_AIRBORNE: u8 = 2;
/// `PlayerState::flags`: aiming down sights.
pub const FLAG_AIMING: u8 = 4;
/// `PlayerState::flags`: reloading.
pub const FLAG_RELOADING: u8 = 8;

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            health: MAX_HEALTH,
            loadout: 0,
            shots: 0,
            steps: 0,
            deaths: 0,
            killer: 0,
            flags: 0,
            rank_points: 0,
            bombs: 0,
            flashes: 0,
        }
    }
}

pub const MAX_HEALTH: f32 = 100.0;

/// Players alternate teams by id: even ids are blue, odd ids are orange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Team {
    Blue,
    Orange,
}

impl Team {
    pub fn of(id: PlayerId) -> Self {
        if id.is_multiple_of(2) {
            Team::Blue
        } else {
            Team::Orange
        }
    }

    pub fn srgb(self) -> [f32; 3] {
        match self {
            Team::Blue => [
                0x2f as f32 / 255.0,
                0x6f as f32 / 255.0,
                0xd6 as f32 / 255.0,
            ],
            Team::Orange => [
                0xe2 as f32 / 255.0,
                0x70 as f32 / 255.0,
                0x2a as f32 / 255.0,
            ],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Team::Blue => "Blue",
            Team::Orange => "Orange",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Team::Blue => 0,
            Team::Orange => 1,
        }
    }
}

/// Whether the current match is free-for-all (everyone is everyone's enemy).
/// Set by the host when a match starts and by players when they hear about it.
static FREE_FOR_ALL: AtomicBool = AtomicBool::new(false);

pub fn set_free_for_all(on: bool) {
    FREE_FOR_ALL.store(on, Ordering::Relaxed);
}

pub fn free_for_all() -> bool {
    FREE_FOR_ALL.load(Ordering::Relaxed)
}

/// Whether paint from `a` can hurt `b`: different teams, or anyone else in
/// free-for-all.
pub fn enemies(a: PlayerId, b: PlayerId) -> bool {
    a != b && (free_for_all() || Team::of(a) != Team::of(b))
}

/// The three ways to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum GameMode {
    /// Blue vs orange, most splats in 10 minutes wins.
    #[default]
    TeamSplat,
    /// Everyone for themselves, most splats in 10 minutes wins.
    FreeForAll,
    /// One life per round, first team to win 5 rounds wins.
    Elimination,
}

impl GameMode {
    pub fn name(self) -> &'static str {
        match self {
            GameMode::TeamSplat => "Team Splat",
            GameMode::FreeForAll => "Free-for-All",
            GameMode::Elimination => "Elimination",
        }
    }

    pub fn teams(self) -> bool {
        self != GameMode::FreeForAll
    }
}

/// Which map: the map file's name without `.json`, at most 31 bytes. The
/// built-in maps keep the names they had when this was an enum, so their ids
/// look the same on the wire as before. Small and `Copy` so it can ride in
/// `MatchSettings`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MapId {
    len: u8,
    bytes: [u8; 31],
}

impl MapId {
    pub const SPLAT_PARK: MapId = MapId::new("SplatPark");
    pub const DOCKYARD: MapId = MapId::new("Dockyard");
    pub const RING_ROAD: MapId = MapId::new("RingRoad");
    pub const THREE_LANES: MapId = MapId::new("ThreeLanes");
    pub const PINE_HOLLOW: MapId = MapId::new("PineHollow");

    /// The maps that come with the game (`ink_map::BUILT_IN_IDS` lists the same names).
    #[allow(dead_code)]
    pub const BUILT_IN: [MapId; 5] = [
        MapId::SPLAT_PARK,
        MapId::DOCKYARD,
        MapId::RING_ROAD,
        MapId::THREE_LANES,
        MapId::PINE_HOLLOW,
    ];

    /// An id from a name (cut to 31 bytes, on a character boundary).
    pub const fn new(name: &str) -> Self {
        let src = name.as_bytes();
        let mut len = if src.len() < 31 { src.len() } else { 31 };
        // Don't split a character.
        while len > 0 && len < src.len() && (src[len] & 0xC0) == 0x80 {
            len -= 1;
        }
        let mut bytes = [0u8; 31];
        let mut i = 0;
        while i < len {
            bytes[i] = src[i];
            i += 1;
        }
        Self {
            len: len as u8,
            bytes,
        }
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

impl Default for MapId {
    fn default() -> Self {
        MapId::SPLAT_PARK
    }
}

impl std::fmt::Debug for MapId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Display for MapId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for MapId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MapId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<'de, str>::deserialize(d)?;
        Ok(MapId::new(&name))
    }
}

/// How fast everyone moves, picked by the host when creating a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MovementSpeed {
    /// Slower movement; the jump direction is locked once you leave the ground.
    #[default]
    Normal,
    /// The original fast movement and jump, with full control in the air.
    SuperDuper,
}

impl MovementSpeed {
    pub const ALL: [MovementSpeed; 2] = [MovementSpeed::Normal, MovementSpeed::SuperDuper];

    pub fn name(self) -> &'static str {
        match self {
            MovementSpeed::Normal => "Normal",
            MovementSpeed::SuperDuper => "Super Duper Speed",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            MovementSpeed::Normal => "Grounded pace. No steering mid-air",
            MovementSpeed::SuperDuper => "Fast run, big jumps, air control",
        }
    }
}

/// What the host picked: mode, map, movement, and whether it counts for ranked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchSettings {
    pub mode: GameMode,
    pub map: MapId,
    /// Ranked (Elimination only).
    pub ranked: bool,
    /// How fast players move and jump.
    pub movement: MovementSpeed,
}

/// Where a match is at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Phase {
    /// Not enough players yet: play freely, nothing counts.
    #[default]
    Warmup,
    /// Everyone's back at spawn and held still until it starts.
    Countdown,
    Live,
    /// Elimination: a round just ended.
    RoundOver,
    /// The match is decided; results are up until the next one starts.
    MatchOver,
}

/// Who won a round or a match.
pub const WIN_NONE: u8 = 0;
pub const WIN_BLUE: u8 = 1;
pub const WIN_ORANGE: u8 = 2;
pub const WIN_DRAW: u8 = 3;
/// Free-for-all: `winner` is `WIN_PLAYER + id`.
pub const WIN_PLAYER: u8 = 10;

/// The match as the host sees it, sent to everyone a few times a second.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MatchState {
    pub settings: MatchSettings,
    pub phase: Phase,
    /// Seconds left in this phase (match clock while live).
    pub time_left: f32,
    /// Counts matches: a new one means everyone respawns and scores reset.
    pub match_id: u32,
    /// Elimination: the round being played (from 1).
    pub round: u32,
    /// Blue and orange: splats (Team Splat) or rounds won (Elimination).
    pub score: [u32; 2],
    /// Last round's winner (`WIN_*`).
    pub round_winner: u8,
    /// The match winner once it's over (`WIN_*`, `WIN_PLAYER + id` in free-for-all).
    pub winner: u8,
    /// Per player: [id, splats, times splatted].
    pub board: Vec<[u32; 3]>,
    /// Bumped whenever everyone goes back to their spawn (a countdown starts).
    pub resets: u32,
}

impl MatchState {
    pub fn kills(&self, id: PlayerId) -> u32 {
        self.board.iter().find(|l| l[0] == id).map_or(0, |l| l[1])
    }

    pub fn deaths(&self, id: PlayerId) -> u32 {
        self.board.iter().find(|l| l[0] == id).map_or(0, |l| l[2])
    }

    /// Whether `id` won the match that just ended.
    pub fn is_winner(&self, id: PlayerId) -> bool {
        if self.phase != Phase::MatchOver {
            return false;
        }
        match self.settings.mode {
            GameMode::FreeForAll => self.winner == WIN_PLAYER.saturating_add(id.min(200) as u8),
            _ => match Team::of(id) {
                Team::Blue => self.winner == WIN_BLUE,
                Team::Orange => self.winner == WIN_ORANGE,
            },
        }
    }

    /// Players hold still (countdown, results).
    pub fn frozen(&self) -> bool {
        matches!(self.phase, Phase::Countdown | Phase::MatchOver)
    }

    /// Whether eliminated players come back on their own.
    pub fn respawns(&self) -> bool {
        !(self.settings.mode == GameMode::Elimination
            && matches!(self.phase, Phase::Live | Phase::RoundOver))
    }

    /// Whether this phase's splats count.
    pub fn counting(&self) -> bool {
        self.phase == Phase::Live
    }
}

/// Elimination: rounds needed to win the match.
pub const ROUNDS_TO_WIN: u32 = 5;
/// Team Splat and Free-for-All length.
pub const MATCH_SECONDS: f32 = 600.0;

/// A confirmed paintball/water hit reported by the shooter and relayed to the victim.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HitEvent {
    /// Monotonically increasing per-attacker id so UDP retries never apply twice.
    pub hit_id: u32,
    pub attacker: PlayerId,
    pub victim: PlayerId,
    pub damage: f32,
    /// Projectile velocity at impact (for hit splash normal and ragdoll push).
    pub velocity: [f32; 3],
    /// World-space impact point (for hit splash particles on the victim).
    pub point: [f32; 3],
    pub water: bool,
    /// Landed in the head zone (damage already includes the bonus).
    #[serde(default)]
    pub headshot: bool,
    /// A flashbang went off next to the victim: no damage, paint over their
    /// screen instead.
    #[serde(default)]
    pub flash: bool,
}

/// Messages sent from a joining client to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    UpdateTransform {
        position: [f32; 3],
        yaw: f32,
        #[serde(default)]
        state: PlayerState,
    },
    /// The client's projectile struck another player on their screen.
    ReportHit(HitEvent),
    /// "Still here", every couple of seconds, so a host notices a player that vanished.
    Ping,
    /// The player is leaving (sent before closing).
    Leave,
    /// The name to show for this player (their Portside name). Sent once
    /// connected, and over UDP again every couple of seconds in case it's lost.
    SetName(String),
}

/// Messages sent from the host to connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome {
        your_id: PlayerId,
        players: Vec<PlayerSnapshot>,
        /// The match in progress (mode, map, scores).
        #[serde(default)]
        match_state: Option<MatchState>,
    },
    PlayerJoined(PlayerSnapshot),
    PlayerLeft {
        id: PlayerId,
    },
    WorldState {
        players: Vec<PlayerSnapshot>,
    },
    /// Another player's shot hit this client.
    PlayerHit(HitEvent),
    /// The host is closing the game: everyone goes back to the menu.
    HostLeaving,
    /// The match clock, phase and scores.
    Match(MatchState),
    /// Eliminated players' bodies as the host simulates them.
    Ragdolls(RagdollFrame),
    /// Some players' names (`[id, name]`), added to what's already known.
    /// Players without one show as "Player N". Sent whenever a name changes
    /// and every couple of seconds, in pieces that fit a datagram.
    Names(Vec<(PlayerId, String)>),
}

/// Longest name shown, in characters.
pub const MAX_NAME_CHARS: usize = 20;

/// A name fit to show: no control characters, trimmed, at most
/// `MAX_NAME_CHARS`. `None` when nothing is left.
pub fn clean_name(raw: &str) -> Option<String> {
    let cut: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME_CHARS)
        .collect();
    let cut = cut.trim_end().to_string();
    (!cut.is_empty()).then_some(cut)
}

/// Names per `Names` message, so each fits a datagram (20 characters of up
/// to 4 bytes each, plus the id and JSON around it).
pub const NAMES_PER_MESSAGE: usize = 8;

/// Splits a name list into `Names` messages that each fit a datagram.
pub fn names_messages(names: &[(PlayerId, String)]) -> Vec<ServerMessage> {
    names
        .chunks(NAMES_PER_MESSAGE)
        .map(|chunk| ServerMessage::Names(chunk.to_vec()))
        .collect()
}

/// Joints in a ragdoll pose (see `ragdoll.rs`).
pub const RAGDOLL_JOINTS: usize = 13;

/// One body: whose it is, which of their eliminations made it, and where its
/// joints are (centimeters, x/y/z per joint).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagdollPose {
    pub owner: PlayerId,
    pub death: u32,
    pub joints: Vec<i32>,
}

/// Part of the host's ragdolls. `all`, when there, lists every body that
/// exists right now (`[owner, death]`): any other is gone. `bodies` are the
/// poses of the ones that moved (and, now and then, all of them).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RagdollFrame {
    pub all: Option<Vec<[u32; 2]>>,
    pub bodies: Vec<RagdollPose>,
}

/// Splits the host's bodies into frames that each fit a datagram: the first
/// lists every body (and carries one pose), the rest carry two poses each.
pub fn pack_ragdoll_frames(all: Vec<[u32; 2]>, bodies: Vec<RagdollPose>) -> Vec<RagdollFrame> {
    let mut rest = bodies.into_iter();
    let first = RagdollFrame {
        all: Some(all),
        bodies: rest.next().into_iter().collect(),
    };
    let rest: Vec<RagdollPose> = rest.collect();
    std::iter::once(first)
        .chain(rest.chunks(2).map(|chunk| RagdollFrame {
            all: None,
            bodies: chunk.to_vec(),
        }))
        .collect()
}

/// What a joining player sends the host over UDP (one JSON datagram each).
///
/// UDP is what Portside's game sessions carry (through its VOIP TailCat copy),
/// so a hub game runs on this. Datagrams can be lost: `Hello` is resent until
/// the host's `Welcome` arrives, and `WorldState` is sent many times a second.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientDatagram {
    Hello,
    Msg(ClientMessage),
    Keepalive,
    Bye,
}

/// What the host sends a UDP player.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerDatagram {
    Msg(ServerMessage),
    /// Part of the world, when all of it doesn't fit one datagram.
    Players {
        players: Vec<PlayerSnapshot>,
    },
    /// Everyone in the game right now, by id (sent with `Players`).
    Roster {
        ids: Vec<PlayerId>,
    },
    Keepalive,
    Bye,
}

/// Keep datagrams under what fits a tunnel packet with room to spare.
pub const MAX_DATAGRAM: usize = 1100;

/// Encodes a host message as one or more datagrams that each fit `MAX_DATAGRAM`.
pub fn encode_server_datagrams(msg: &ServerMessage) -> Vec<Vec<u8>> {
    let whole = serde_json::to_vec(&ServerDatagram::Msg(msg.clone())).unwrap_or_default();
    if whole.len() <= MAX_DATAGRAM {
        return vec![whole];
    }
    let players = match msg {
        ServerMessage::WorldState { players } => players,
        ServerMessage::Welcome { players, .. } => players,
        _ => return vec![whole],
    };
    let mut out = Vec::new();
    if let ServerMessage::Welcome {
        your_id,
        match_state,
        ..
    } = msg
    {
        let w = ServerMessage::Welcome {
            your_id: *your_id,
            players: Vec::new(),
            match_state: match_state.clone(),
        };
        out.extend(serde_json::to_vec(&ServerDatagram::Msg(w)));
    }
    let ids = players.iter().map(|p| p.id).collect();
    out.extend(serde_json::to_vec(&ServerDatagram::Roster { ids }));
    for chunk in players.chunks(4) {
        out.extend(serde_json::to_vec(&ServerDatagram::Players {
            players: chunk.to_vec(),
        }));
    }
    out
}

/// Deterministic color palette so each player stands out clearly on the baseplate.
pub fn player_color(id: PlayerId) -> [f32; 3] {
    const PALETTE: &[[f32; 3]] = &[
        [0.24, 0.62, 0.96], // Sky blue (Host)
        [0.95, 0.36, 0.36], // Coral red
        [0.25, 0.80, 0.52], // Emerald
        [0.96, 0.72, 0.22], // Amber
        [0.68, 0.44, 0.96], // Violet
        [0.18, 0.82, 0.86], // Cyan
        [0.95, 0.46, 0.74], // Rose
        [0.64, 0.86, 0.26], // Lime
    ];
    PALETTE[(id as usize) % PALETTE.len()]
}

/// Where a player (re)spawns: on their team's pad at one end of the field,
/// spread out a little so teammates don't overlap.
pub fn spawn_position(id: PlayerId) -> [f32; 3] {
    let side = match Team::of(id) {
        Team::Blue => 1.0,
        Team::Orange => -1.0,
    };
    let slot = (id / 2) as f32;
    let x = (slot % 5.0 - 2.0) * 1.6;
    let z = side * (SPAWN_PAD_Z + (slot / 5.0).floor() * 1.4);
    [x, 1.0, z]
}

/// How far from the middle the two spawn pads sit (blue +Z, orange -Z).
pub const SPAWN_PAD_Z: f32 = 24.0;

/// Writes a length-prefixed JSON frame onto an async stream.
pub async fn write_frame<W, T>(writer: &mut W, msg: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes =
        serde_json::to_vec(msg).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "outbound frame exceeds maximum size",
        ));
    }
    let len = bytes.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// Reads a length-prefixed JSON frame from an async stream.
/// Returns `Ok(None)` when the peer cleanly closes the connection.
pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid frame length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let msg = serde_json::from_slice(&buf)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Kit  ").as_deref(), Some("Kit"));
        assert_eq!(clean_name("Jo\n\u{7}"), Some("Jo".to_string()));
        assert_eq!(clean_name("   "), None);
        assert_eq!(clean_name("").as_deref(), None);
        let long = clean_name(&"a".repeat(50)).unwrap();
        assert_eq!(long.chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn name_messages_fit_datagrams() {
        let names: Vec<(PlayerId, String)> = (0..40)
            .map(|id| (1_000_000 + id, "\u{1F600}".repeat(MAX_NAME_CHARS)))
            .collect();
        let msgs = names_messages(&names);
        let mut seen = 0;
        for msg in &msgs {
            for d in encode_server_datagrams(msg) {
                assert!(d.len() <= MAX_DATAGRAM, "datagram of {} bytes", d.len());
            }
            if let ServerMessage::Names(n) = msg {
                seen += n.len();
            }
        }
        assert_eq!(seen, 40);
    }

    #[test]
    fn ragdoll_frames_fit_datagrams() {
        let all: Vec<[u32; 2]> = (0..24).map(|i| [1000 + i, 123_456]).collect();
        let bodies: Vec<RagdollPose> = (0..24)
            .map(|i| RagdollPose {
                owner: 1000 + i,
                death: 123_456,
                joints: vec![-123_456; RAGDOLL_JOINTS * 3],
            })
            .collect();
        let frames = pack_ragdoll_frames(all, bodies);
        assert!(frames[0].all.as_ref().is_some_and(|a| a.len() == 24));
        assert_eq!(frames.iter().map(|f| f.bodies.len()).sum::<usize>(), 24);
        for frame in frames {
            for d in encode_server_datagrams(&ServerMessage::Ragdolls(frame)) {
                assert!(d.len() <= MAX_DATAGRAM, "datagram of {} bytes", d.len());
            }
        }
    }

    #[test]
    fn big_world_state_is_split_into_datagrams_that_fit() {
        let players: Vec<_> = (0..40)
            .map(|id| PlayerSnapshot {
                id,
                position: [123.456, 1.0, -98.765],
                yaw: 2.71,
                color: player_color(id),
                state: PlayerState {
                    pitch: -0.123_456_7,
                    health: 87.654_32,
                    loadout: u32::MAX,
                    shots: 123_456,
                    steps: 654_321,
                    deaths: 1234,
                    killer: 39,
                    flags: 15,
                    rank_points: 1234,
                    bombs: 255,
                    flashes: 255,
                },
            })
            .collect();
        let small = encode_server_datagrams(&ServerMessage::WorldState {
            players: players[..2].to_vec(),
        });
        assert_eq!(small.len(), 1);
        let big = encode_server_datagrams(&ServerMessage::WorldState { players });
        assert!(big.len() > 2);
        let mut seen = 0;
        for d in &big {
            assert!(d.len() <= MAX_DATAGRAM, "datagram of {} bytes", d.len());
            if let ServerDatagram::Players { players } = serde_json::from_slice(d).unwrap() {
                seen += players.len();
            }
        }
        assert_eq!(seen, 40);
    }
}

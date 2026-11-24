use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Virtual TCP port used inside the Tailcat tunnel for game traffic.
pub const GAME_PORT: u16 = 7777;

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
}

/// Messages sent from a joining client to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    UpdateTransform { position: [f32; 3], yaw: f32 },
}

/// Messages sent from the host to connected clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome {
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

/// Computes a slightly offset spawn position around the center pad so players do not overlap.
pub fn spawn_position(id: PlayerId) -> [f32; 3] {
    if id == 0 {
        return [0.0, 1.0, 0.0];
    }
    let angle = (id as f32) * 1.1;
    let radius = 3.0 + ((id / 6) as f32) * 1.5;
    [angle.cos() * radius, 1.0, angle.sin() * radius]
}

/// Writes a length-prefixed JSON frame onto an async stream.
pub async fn write_frame<W, T>(writer: &mut W, msg: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(msg)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
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

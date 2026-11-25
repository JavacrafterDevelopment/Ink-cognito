# Physics & Shooting Architecture

This document explains how player movement, arena collision, projectile ballistics, hit registration, and multiplayer synchronization work in **Ink-cognito**.

---

## 1. Multiplayer Architecture (Listen-Server)

Ink-cognito uses a **host-relayed listen-server** model — there is no separate dedicated server binary. Whoever clicks **Create Game** becomes Player `0` (Blue team) and runs both the game client and the networking relay:

- **Transports** (`src/net.rs`, `src/protocol.rs`):
  - **Portside Hub (UDP)**: Local UDP socket forwarded through Portside's session tunnel (`ClientDatagram` / `ServerDatagram`). Large world states are automatically chunked under `MAX_DATAGRAM` (1100 bytes).
  - **Tailcat (TCP)**: Userspace WireGuard + DERP tunnel with NAT hole-punching (`ClientMessage` / `ServerMessage`, length-prefixed JSON frames on virtual port `7777`).
- **Tick Rate**:
  - Clients send `UpdateTransform` (position, yaw, pitch, health, loadout, shot/death counters) at **50 Hz** (`20 ms` interval), and flush **immediately** whenever `shots` or `deaths` increments so firing and eliminations show up without waiting for the next tick.
  - The host broadcasts `WorldState` snapshots to all connected clients at **50 Hz** (`20 ms`).

---

## 2. Player Movement & Arena Physics

Player physics run locally on each player's machine every frame (`move_local_player` in `src/world.rs`, colliders in `src/arena.rs`):

- **Player Dimensions**:
  - Standing cylinder with radius `0.38 m` (`PLAYER_RADIUS`), total height `1.8 m`, center at `y = 0.9 m` (`PLAYER_GROUND_Y`), and first-person camera eye at `center.y + 0.72 m` (`EYE_OFFSET`).
- **Movement Speeds**:
  - Walk: `6.5 m/s` (`WALK_SPEED`)
  - Sprint (`Shift`, disabled while aiming): `10.0 m/s` (`SPRINT_SPEED`)
  - Multiplied by the active loadout's `mobility` stat, and multiplied by `0.6×` while aiming down sights (`Right Click`).
- **Jumping & Gravity**:
  - Jump impulse: `7.2 m/s` (`JUMP_SPEED`) when grounded.
  - Gravity: `22.0 m/s²` (`GRAVITY`).
  - Frame `dt` is clamped to `0.05 s` to keep physics stable during frame hitches.
- **Arena Collision (`Colliders::resolve_player`)**:
  - Every piece's collision is a few simple shapes fitted to its model (`tools/colliders`): axis-aligned boxes (`Shape::Box`), upright cylinders (`Shape::Cylinder`), boxes turned around the vertical (`Shape::Oriented`), and convex shapes (`Shape::Hull`) for everything else: tilted pieces, ramps at any angle, rods and fitted hulls.
  - Overlapping geometry pushes the player smoothly outward on the XZ plane. For a convex shape the push comes from its outline cut at a few heights the player's body spans.
  - If the player's feet are near or above a bunker's top (`feet >= max.y - 0.25`), the top acts as a standable floor surface. Convex shapes use the height of their surface under the player: flat tops take the same 0.25 m step, walkable slopes (ramps, roofs) up to 0.45 m, and steep faces none.
  - Paint hits convex shapes on their faces (ramps included), and ragdoll joints are pushed out through the nearest face.
  - Finally, XZ coordinates are clamped inside the `60 m × 60 m` field nets (`±(FIELD_HALF - PLAYER_RADIUS - 0.15)`).

---

## 3. Remote Player Sync & Prediction

Raw network snapshots arrive at 50 Hz with variable latency. Pure exponential smoothing lags behind moving targets, so `src/world.rs` combines velocity estimation with bounded dead-reckoning:

1. **Velocity Estimation (`sync_remote_players`)**:
   - Whenever a new position arrives for a remote player, velocity is computed as `delta / dt_snap` (clamped to `16 m/s`) and blended into `remote.net_velocity`.
   - Large jumps (`> 6.0 m`, such as a respawn teleport) snap immediately and zero out velocity instead of sliding across the field.
2. **Prediction & Smoothing (`interpolate_remote_players`)**:
   - Between snapshots, the target position is projected forward along `net_velocity` by up to **65 ms** to offset smoothing and tick latency.
   - If no new position arrives for `> 90 ms`, `net_velocity` damps quickly toward zero so stopped players do not overshoot or drift into cover.
   - The visible transform smoothly blends toward the predicted position with stiffness `22.0` (`1 - exp(-22 * dt)`).

---

## 4. Shooting & Projectile Ballistics

Weapon behavior is driven by `WeaponStats` computed from the player's `Loadout` (`src/loadout.rs`) and simulated in `src/combat.rs`:

- **Firing (`fire_local`)**:
  - Respects weapon `fire_interval`, magazine `capacity`, and `reload` time.
  - Spread scales based on movement (`0.6×` while still, `1.0×` while moving) and ADS blend (`aim_spread` interpolated by `aim_blend`).
  - Each shot uses a deterministic xorshift spread perturbation seeded by `(player_id, shot_number)` (`shot_direction`).
- **Projectile Flight (`move_projectiles`)**:
  - Every frame, each active paintball or water droplet accelerates downward by its weapon `gravity` and steps along the swept segment `from -> to` (so fast balls never tunnel through thin bunkers or players).
  - Lifetime is capped at `3.0 s`.
- **Collision Priority**:
  - Along `from -> to`, `Colliders::segment_hit` finds the earliest hit on the ground (`y = 0`), boxes, or cylinders (`best_t`).
  - Enemy player hitboxes are tested via closest segment-to-segment distance against a standing vertical capsule (`PLAYER_HIT_RADIUS = 0.44 m`). Teammates are skipped (friendly paint passes through).
  - Whichever hit has the smallest `t` along `0..=1` wins — so cover properly blocks shots.
- **Impacts & Visuals**:
  - World hits spawn paint splat decals (or vertical drip decals on walls, capped at `MAX_DECALS = 350`), droplet/puff particles, and distance-attenuated splat/bunker sounds. Water decals fade out over 5 seconds.

---

## 5. Hit Registration & Damage Flow

Ink-cognito uses **shooter-confirmed, host-validated hit registration** ("favor the shooter"). This guarantees that when you hit a moving enemy on your screen and see a hitmarker, they reliably take damage even under network latency.

```text
Shooter (Client A)                  Host (Relay & Validator)                Victim (Client B)
──────────────────                  ────────────────────────                ─────────────────
1. Local ball hits B's hitbox
   -> Hitmarker + hit tick SFX
   -> Paint splash VFX
   -> Send ReportHit(hit_id) ──────> 2. Validate & deduplicate:
      (TCP once; UDP 3x burst)          - attacker == sender_id
                                        - enemy team only
                                        - 0 < damage <= 150
                                        - HitTracker(attacker, hit_id)
                                        Relay PlayerHit(hit) ─────────────> 3. Deduplicate:
                                        (TCP once; UDP 3x burst)               - HitBookkeeping
                                                                               - Verify enemy + alive
                                                                               Apply take_damage()
                                                                               - Health -= damage
                                                                               - Damage flash + SFX
                                                                               - If 0 HP: ragdoll,
                                                                                 deaths++, respawn timer
                                     4. Broadcast updated health/deaths <── Send UpdateTransform
                                        to all players (immediate flush     (flushed immediately on
                                        on elimination)                      death)
```

### Key Details

- **No Ghost Hitmarkers**: Because the shooter reports the exact `HitEvent` (`hit_id`, `attacker`, `victim`, `damage`, `velocity`, `point`, `water`) the instant their projectile connects, moving targets cannot dodge a shot after it already landed on the shooter's screen.
- **No Dropped Hits Over UDP**: On Portside (UDP), both client→host `ReportHit` and host→victim `PlayerHit` are sent immediately plus two quick redundant retries (`+35 ms`, `+75 ms`).
- **No Double Damage**:
  - Every hit carries a monotonically increasing `hit_id` per attacker.
  - Both the host (`HitTracker`, 512-entry sliding window) and victim (`HitBookkeeping`, 512-entry sliding window) ignore any duplicate `(attacker, hit_id)`.
  - Remote visual projectiles spawned by `fire_remote` (triggered by the shooter's `shots` counter) only render tracers, muzzle flash, and world paint splats — they despawn visually if they touch the local player without applying separate damage.
- **Health, Regen & Elimination (`take_damage`, `regen_and_respawn`)**:
  - Max health is `100.0`.
  - After `7.0 s` without taking damage (`REGEN_DELAY`), health regenerates at `14.0 HP/s` (`REGEN_RATE`).
  - At `0 HP`, the player's `deaths` counter increments, `killer` is recorded, a physics ragdoll spawns pushed along the impact velocity, the death camera orbits for `5.0 s` (`RESPAWN_TIME`), and the player respawns with full health and ammo on their team's spawn pad.

---

## 6. Trust Model & Trade-offs

- **Why shooter-confirmed?** In a peer-hosted game over Tailcat/Portside without a dedicated server running full lag-compensated rewind physics, victim-side collision causes severe ghost hitmarkers on strafing players. Favor-the-shooter makes aiming feel crisp and predictable at normal internet pings.
- **What the host enforces**: The host always overwrites `hit.attacker` with the authenticated sender's `PlayerId`, rejects self-hits and friendly fire, clamps damage to `(0, 150]`, and deduplicates hit IDs before relaying to the victim.

# Ink-cognito

Ink-cognito is a 3D multiplayer paintball game written in Rust with [Bevy](https://bevyengine.org/).
Players spawn on two teams (blue and orange) at opposite ends of an airball field and
splat each other with paintball markers or water guns. Networking runs
over [TailCat](https://crates.io/crates/tailcat): one player hosts and shares a TailCat
address, and everyone else joins by pasting that address. No port forwarding is needed.

The game is in early development. Each player has 100 health; health starts coming back
7 seconds after the last hit. An eliminated player drops as a ragdoll (gone after 15 seconds)
and respawns 5 seconds later, at a spawn away from enemies.

### Modes

| Mode | Rules |
| --- | --- |
| Team Splat | Blue vs orange for 10 minutes. The team with the most splats wins. |
| Free-for-All | Everyone for themselves for 10 minutes. The player with the most splats wins. |
| Elimination | One life per round (2 minute rounds). The first team to win 5 rounds wins. |
| Ranked Elimination | Elimination for rank points (RP). |

A match waits in warmup until there is someone to play against, then everyone goes back to
spawn for a 5 second countdown. When the match ends, the top three splatters celebrate on a podium for two minutes, then
everyone goes back to the main menu (or click **Main Menu** to leave sooner).

**Ranked:** a win gives +30 RP (plus 2 per splat, up to +10), a loss takes 20, and quitting a
ranked match before it's decided counts as a loss. Ranks are Bronze, Silver, Gold, Platinum
and Diamond (three divisions of 100 RP each, from 0, 300, 600, 900 and 1200 RP) and Champion
(1500 RP). Your RP is saved on your computer next to your loadout.

### Maps

| Map | Style |
| --- | --- |
| Splat Park | The original airball field with inflatable bunkers. |
| Dockyard | A small, tight container yard packed with cover. |
| Ring Road | A wide outer loop around two walled blocks, joined by cross-paths. |
| Three Lanes | A middle lane and two flanks (the Yard and the Woods), linked by connectors. |
| Pine Hollow | Woods with a raised skybridge in the middle and perches to hold. |

## How to play

1. Start the game. The main menu has **Play**, **Join by Address**, **Loadout**, **Options**
   and **Quit**, your rank card, and the games in your Portside hub.
2. **Host:** click **Play**, pick a mode and a map (the map shows behind the menu), and
   click **Create Match**. Once hosting starts, the in-game panel shows your TailCat address
   (`tc...`). Copy it from the Esc menu and send it to your friends.
3. **Join:** click **Join by Address**, paste the host's address (Ctrl+V or **Paste Clipboard**),
   and press Enter or click join.
4. Press **Esc** in game for the menu: Resume, Loadout, Copy Tailcat Address (host),
   Leave Game and Quit. If the host leaves, the game ends for everyone.
5. **Loadout** (main menu or Esc menu): build a paintball marker from a body and
   barrel, hopper, sight, stock, air tank, foregrip and camo skin, or pick a water gun, and
   choose a crosshair. The Breacher body is a paint shotgun (6 balls x 13 damage a shot) and the
   Longshot is a sniper. Your loadout is saved between runs.
6. **Unlocks:** every marker body levels up on its own. Its camos unlock at 25, 50, 100, 150,
   200, 300, 400 and 500 splats with that body, and its attachments unlock with splats or
   challenges (crouching splats, splats from 25 m+, hip-fire splats and aimed splats). Only
   splats in a live match count. Locked parts show what they need and how far along you are.

### Controls

| Action | Keys |
| --- | --- |
| Move | W A S D or arrow keys |
| Look | Mouse |
| Sprint | Shift |
| Crouch | C or Left Ctrl |
| Jump | Space |
| Shoot | Left mouse button (hold for automatic fire) |
| Aim down sights | Right mouse button |
| Reload | R |
| Scoreboard | Tab (hold) |
| Menu | Esc |

## Building from source

You need a recent stable Rust toolchain (the crate uses edition 2024, so Rust 1.85 or newer).
Install it from [rustup.rs](https://rustup.rs/).

```sh
git clone https://github.com/JavacrafterDevelopment/Ink-cognito.git
cd Ink-cognito
cargo run --release
```

The first build compiles Bevy and takes a while. Dev builds (`cargo run`) are already
tuned for playable frame rates, but `--release` gives the best performance.

On Linux, Bevy needs a few system libraries. On Debian or Ubuntu:

```sh
sudo apt install pkg-config libasound2-dev libudev-dev libx11-dev libxkbcommon-x11-0 libwayland-dev
```

## Project layout

| File | What it does |
| --- | --- |
| `src/main.rs` | App setup, window, and the menu / in-game states |
| `src/menu.rs` | Main menu, play screen (mode and map), join screen, and the in-game info panel |
| `src/ui_kit.rs` | The UI kit: 9-slice buttons and panels, fonts |
| `src/music.rs` | Main menu music: cycles through the `assets/music/Menu_*` tracks |
| `src/pause.rs` | The Esc menu |
| `src/loadout.rs` | Guns, attachments, water guns and their stats |
| `src/loadout_ui.rs` | The loadout screen with a 3D preview |
| `src/maps.rs` | The five maps: models, collision shapes, spawns and minimap shapes |
| `src/arena.rs` | Collision shapes (boxes, cylinders, ramps) and how players and paint hit them |
| `src/game_mode.rs` | Match rules: phases, clock, scores, rounds, respawns (run by the host) |
| `src/ranked.rs` | Rank points, tiers and the ranked save file |
| `src/progression.rs` | Per-body splat records, camo and attachment unlocks, unlock pop-ups |
| `src/podium.rs` | The end-of-match podium and the trip back to the main menu |
| `src/world.rs` | Player movement and camera |
| `src/player_model.rs` | Rigged player models, animations, and the gun in their hands |
| `src/combat.rs` | Shooting, paint effects, health, regeneration and respawns |
| `src/ragdoll.rs` | Ragdolls for eliminated players |
| `src/hud.rs` | Score header, banners, health, ammo, crosshair, minimap, killfeed and scoreboard |
| `src/net.rs` | TailCat hosting and joining, running on a background Tokio runtime |
| `src/protocol.rs` | Message types and length-prefixed JSON framing |

## Assets

`assets/` holds the LowPoly Armory models, textures and sounds (see
`assets/ASSETS-README.txt`), the menu music in `assets/music`, and the logo and key art in
`assets/branding`. The UI kit is in `assets/textures/ui-kit`, camo skins in
`assets/textures/skins`, the map kit in `assets/models/map-kit`, and the Anton and Archivo
Black fonts (SIL Open Font License) in `assets/fonts`. Bevy loads them from the `assets` folder next to the executable.

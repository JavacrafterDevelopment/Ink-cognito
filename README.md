# Ink-cognito

Ink-cognito is a 3D multiplayer paintball game written in Rust with [Bevy](https://bevyengine.org/).
Players spawn on two teams (blue and orange) at opposite ends of an airball field and
splat each other with paintball markers or water guns. Networking runs
over [TailCat](https://crates.io/crates/tailcat): one player hosts and shares a TailCat
address, and everyone else joins by pasting that address. No port forwarding is needed.

The game is in early development. Each player has 100 health; health starts coming back
7 seconds after the last hit. An eliminated player drops as a ragdoll (gone after 15 seconds)
and respawns on their team's pad 5 seconds later.

## How to play

1. Start the game. The main menu has **Create Game** and **Join Game**.
2. **Host:** click **Create Game**. Once hosting starts, the in-game panel shows your
   TailCat address (`tc...`). Click it to copy it and send it to your friends.
3. **Join:** click **Join Game**, paste the host's address (Ctrl+V or **Paste Clipboard**),
   and press Enter or click join.
4. Press **Esc** in game for the menu: Resume, Loadout, Copy Tailcat Address (host),
   Leave Game and Quit. If the host leaves, the game ends for everyone.
5. **Loadout** (main menu or Esc menu): build a paintball marker from a body and
   barrel, hopper, sight, stock, air tank and foregrip, or pick a water gun, and choose a
   crosshair. Your loadout is saved between runs.

### Controls

| Action | Keys |
| --- | --- |
| Move | W A S D or arrow keys |
| Look | Mouse |
| Sprint | Shift |
| Jump | Space |
| Shoot | Left mouse button (hold for automatic fire) |
| Aim down sights | Right mouse button |
| Reload | R |
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
| `src/menu.rs` | Main menu (logo, key art, UI sounds), join screen, and the in-game info panel |
| `src/music.rs` | Main menu music: cycles through the `assets/music/Menu_*` tracks |
| `src/pause.rs` | The Esc menu |
| `src/loadout.rs` | Guns, attachments, water guns and their stats |
| `src/loadout_ui.rs` | The loadout screen with a 3D preview |
| `src/arena.rs` | The paintball field, its bunkers, and collision shapes |
| `src/world.rs` | Player movement and camera |
| `src/player_model.rs` | Rigged player models, animations, and the gun in their hands |
| `src/combat.rs` | Shooting, paint effects, health, regeneration and respawns |
| `src/ragdoll.rs` | Ragdolls for eliminated players |
| `src/hud.rs` | Health bar, ammo, crosshair, killfeed and the elimination screen |
| `src/net.rs` | TailCat hosting and joining, running on a background Tokio runtime |
| `src/protocol.rs` | Message types and length-prefixed JSON framing |

## Assets

`assets/` holds the LowPoly Armory models, textures and sounds (see
`assets/ASSETS-README.txt`), the menu music in `assets/music`, and the logo and key art in
`assets/branding`. Bevy loads them from the `assets` folder next to the executable.

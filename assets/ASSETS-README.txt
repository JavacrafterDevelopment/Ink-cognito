PAINTBALL GAME ASSETS (Bevy)
============================

Drop the `assets/` folder next to your Cargo.toml — Bevy loads from there by default.
Everything uses formats Bevy reads out of the box: .glb (models), .png (textures), .ogg (audio).
preview_textures.png and preview_models.png show everything at a glance.

FOLDER MAP
assets/models/
  guns-v2/bodies, guns-v2/attachments/*, guns-v2/factory-builds   modular paintball guns (Armory V2: textured, see-through sights, ads_eye nodes) (see LowPoly Armory README for mount points)
  players/            blue + orange rigged players (Idle + Walk animations)
  water-guns-v2/      minnow, tidal, monsoon (V2: translucent water with slosh morph targets)
  projectiles/        paintball_{blue,orange,yellow,pink} (real 17.3 mm size), paintball_shell_shards, water_balloon
  bunkers/            inflatable airball set: can, mini_can, cake, dorito, brick, snake, temple, tall_wall, tree
  props/              barrel, crate, sandbag_wall, netting_fence_section, field_cone,
                      flag_blue/orange (pole + base), spawn_pad_blue/orange
  pickups/            pod_pickup, paint_case, air_refill_station
assets/textures/
  splats/             splat_01-08 (05-08 are stretched for angled hits), drip_01-04 for walls,
                      splat_atlas_4x3 (all 12 in one 2048x1536 sheet, 512px cells)
  particles/          droplet, soft_puff, smoke_puff, spark_star, ring_shockwave,
                      splash_streak, shell_fragment, air_puff_cone
  flipbooks/          paint_burst_4x4, air_puff_4x4, water_spray_4x4 (16 frames, 128px cells, read left->right, top->bottom)
  ui/                 crosshair_{cross,dot,circle}, hit_marker, team_blue, team_orange, team_shield_white,
                      ammo_pod, ammo_ball, flag, eliminated, timer, splat_icon, meter_frame
  ground/             turf_tile, dirt_tile (seamless 512px), netting_tile (alpha)
assets/audio/
  weapons/            marker_shot_01-03, pump_cycle, hopper_rattle, pod_reload, tank_hiss, tank_screw_in, dry_fire
  impacts/            splat_surface_01-03, splat_player_hit, ball_bounce, hit_bunker_inflatable, hit_wood, hit_metal
  water/              water_squirt, water_stream_loop (loops), water_pump, water_splash
  game/               round_start_horn, whistle, countdown_beep, countdown_go, flag_captured, eliminated_buzzer
  ui/                 ui_click, ui_hover, ui_confirm, hit_marker_tick
  footsteps/          step_grass_01-04, step_dirt_01-02
src/
  paintball_fx.rs     drop-in plugin: flying paintballs with gravity, splat decals, droplet + mist
                      particles, shot/splat sounds, automatic cleanup of old splats
  main.rs             small example scene (turf, bunkers, flags, both players; left/right click to shoot)
fx_recipes.json       numbers for every effect (sizes, counts, lifetimes, speeds, which texture/sound)

TINTING
Splats, particles, flipbooks and most UI icons are white/grey so one texture works for every team:
set StandardMaterial.base_color (3D) or ImageNode color / Sprite color (UI) to the team colour.
Blue #2f6fd6, Orange #e2702a. Splats carry subtle light/dark shading so they read as glossy paint.

BEVY NOTES
- The Rust code targets Bevy 0.18 and only uses core Bevy (no extra crates). It was not compiled
  here, so on another Bevy version you may need small renames (e.g. GltfAssetLabel, SceneRoot,
  Time::delta_secs). Cargo.toml:   bevy = "0.18"
- Collision: update_projectiles only hits the ground plane (y = 0) as a demo. Swap in a raycast
  from your physics crate (Avian or Rapier) and call spawn_paint_impact(point, normal, team).
- Decals are alpha-blended quads with a depth bias. On Bevy 0.16+ you can switch to the built-in
  ForwardDecal component for decals that wrap onto curved bunkers.
- For GPU particles at scale, port fx_recipes.json to the bevy_hanabi crate.
- Player animations: load the clips with GltfAssetLabel::Animation(0) ("Idle") / (1) ("Walk") and
  play them with AnimationGraph + AnimationPlayer (see Bevy's animated_mesh example).
- Audio is mono .ogg, 44.1 kHz. For positional sound add SpatialAudioSink / spatial PlaybackSettings.

All sounds are synthesised and all art is procedurally made for this project, so you can use and
modify them freely in your game.

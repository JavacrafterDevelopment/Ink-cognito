//! Main-menu music: cycles through the "Menu_..." tracks, starting at a
//! random one, and shows what's playing.

use crate::{AppState, menu::TEXT_MUTED};
use bevy::{audio::Volume, prelude::*};

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_playlist)
            .add_systems(OnEnter(AppState::MainMenu), spawn_now_playing)
            .add_systems(Update, play_next_track.run_if(in_state(AppState::MainMenu)));
    }
}

/// Every file under assets/music whose name starts with "Menu".
const TRACKS: &[&str] = &[
    "music/Menu_Palms_at_Midnight.ogg",
    "music/Menu_Plaza_At_Midnight.ogg",
    "music/Menu_The_Lobby_Charge.ogg",
];

const MUSIC_VOLUME: f32 = 0.45;

#[derive(Resource)]
struct Playlist {
    tracks: Vec<(String, Handle<AudioSource>)>,
    next: usize,
}

/// The track playing now (removed by Bevy when it ends, or on leaving the menu).
#[derive(Component)]
struct MenuTrack;

#[derive(Component)]
struct NowPlayingText;

fn track_title(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.strip_suffix(".ogg").unwrap_or(file);
    stem.trim_start_matches("Menu_").replace('_', " ")
}

fn setup_playlist(mut commands: Commands, assets: Res<AssetServer>) {
    let tracks: Vec<_> = TRACKS
        .iter()
        .map(|p| (track_title(p), assets.load(*p)))
        .collect();
    // A different first track each launch.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    let next = if tracks.is_empty() {
        0
    } else {
        nanos % tracks.len()
    };
    commands.insert_resource(Playlist { tracks, next });
}

fn spawn_now_playing(mut commands: Commands, assets: Res<AssetServer>) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(16.0),
                bottom: Val::Px(12.0),
                column_gap: Val::Px(8.0),
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            DespawnOnExit(AppState::MainMenu),
        ))
        .with_children(|row| {
            row.spawn((
                ImageNode::new(assets.load("textures/ui/splat_icon.png"))
                    .with_color(Color::srgb(0.45, 0.82, 1.0)),
                Node {
                    width: Val::Px(18.0),
                    height: Val::Px(18.0),
                    ..default()
                },
                Pickable::IGNORE,
            ));
            row.spawn((
                Text::new(""),
                TextFont::from_font_size(13.0),
                TextColor(TEXT_MUTED),
                Pickable::IGNORE,
                NowPlayingText,
            ));
        });
}

fn play_next_track(
    mut commands: Commands,
    mut playlist: ResMut<Playlist>,
    playing: Query<(), With<MenuTrack>>,
    mut text_q: Query<&mut Text, With<NowPlayingText>>,
) {
    if !playing.is_empty() || playlist.tracks.is_empty() {
        return;
    }
    let i = playlist.next % playlist.tracks.len();
    playlist.next = (i + 1) % playlist.tracks.len();
    let (title, handle) = playlist.tracks[i].clone();
    commands.spawn((
        AudioPlayer::new(handle),
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(MUSIC_VOLUME)),
        MenuTrack,
        DespawnOnExit(AppState::MainMenu),
    ));
    if let Ok(mut text) = text_q.single_mut() {
        text.0 = format!("Now playing: {title}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_track_is_in_assets() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/music");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("Menu") && n.ends_with(".ogg"))
            .map(|n| format!("music/{n}"))
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = TRACKS.iter().map(|s| s.to_string()).collect();
        listed.sort();
        assert_eq!(on_disk, listed);
        assert_eq!(track_title(TRACKS[0]), "Palms at Midnight");
    }
}

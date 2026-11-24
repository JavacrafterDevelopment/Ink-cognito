//! Persistent audio settings and the Options overlay modal.
//! Opens from both the Main Menu and the in-game Escape pause menu.

use crate::{
    Overlay,
    combat::{Fx, play_sfx},
    menu::{
        BG_CARD, BORDER_SUBTLE, BTN_DANGER, BTN_PRIMARY, ButtonStyle, TEXT_MUTED, TEXT_PRIMARY,
        TEXT_STATUS, spawn_button,
    },
};
use bevy::{
    audio::{AudioSink, AudioSinkPlayback, Volume},
    picking::prelude::*,
    prelude::*,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_settings_click)
            .add_observer(on_slider_press)
            .add_observer(on_slider_over)
            .add_systems(Update, (show_settings_screen, sync_sfx_volume));
    }
}

/// Persisted audio volume levels (all in 0.0..=1.0).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub master: f32,
    pub music: f32,
    pub sfx: f32,
    pub ui: f32,
    pub muted: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master: 1.0,
            music: 1.0,
            sfx: 1.0,
            ui: 1.0,
            muted: false,
        }
    }
}

impl AudioSettings {
    pub fn sanitized(mut self) -> Self {
        let clean = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        self.master = clean(self.master);
        self.music = clean(self.music);
        self.sfx = clean(self.sfx);
        self.ui = clean(self.ui);
        self
    }

    pub fn effective_music(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.master * self.music).clamp(0.0, 1.0)
        }
    }

    pub fn effective_sfx(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.master * self.sfx).clamp(0.0, 1.0)
        }
    }

    pub fn effective_ui(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.master * self.ui).clamp(0.0, 1.0)
        }
    }

    pub fn get(&self, channel: AudioChannel) -> f32 {
        match channel {
            AudioChannel::Master => self.master,
            AudioChannel::Music => self.music,
            AudioChannel::Sfx => self.sfx,
            AudioChannel::Ui => self.ui,
        }
    }

    pub fn set(&mut self, channel: AudioChannel, value: f32) {
        let clamped = (value.clamp(0.0, 1.0) * 20.0).round() / 20.0;
        match channel {
            AudioChannel::Master => self.master = clamped,
            AudioChannel::Music => self.music = clamped,
            AudioChannel::Sfx => self.sfx = clamped,
            AudioChannel::Ui => self.ui = clamped,
        }
        if clamped > 0.0 && self.muted {
            self.muted = false;
        }
    }

    pub fn step(&mut self, channel: AudioChannel, delta: f32) {
        let next = self.get(channel) + delta;
        self.set(channel, next);
    }
}

/// Tag attached to spawned SFX audio entities so active sounds follow live volume edits.
#[derive(Component, Clone, Copy)]
pub struct SfxAudio(pub f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannel {
    Master,
    Music,
    Sfx,
    Ui,
}

impl AudioChannel {
    const ALL: [Self; 4] = [Self::Master, Self::Music, Self::Sfx, Self::Ui];

    fn label(self) -> &'static str {
        match self {
            Self::Master => "Master Volume",
            Self::Music => "Music Volume",
            Self::Sfx => "Sound Effects",
            Self::Ui => "UI Sounds",
        }
    }
}

#[derive(Component)]
struct SettingsRoot;

#[derive(Component, Clone, Copy)]
enum SettingsAction {
    Step(AudioChannel, f32),
    ToggleMute,
    TestSfx,
    ResetDefaults,
    Done,
}

/// Interactive 5%-step segment inside a volume slider bar.
#[derive(Component, Clone, Copy)]
struct SliderSegment {
    channel: AudioChannel,
    value: f32,
}

fn save_path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|h| {
            let mut p = PathBuf::from(h);
            if cfg!(target_os = "macos") {
                p.push("Library/Application Support");
            } else {
                p.push(".config");
            }
            p
        })
    })?;
    Some(base.join("Ink-cognito").join("settings.json"))
}

pub fn load_saved_settings() -> AudioSettings {
    save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<AudioSettings>(&bytes).ok())
        .map(AudioSettings::sanitized)
        .unwrap_or_default()
}

pub fn save_settings(settings: &AudioSettings) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(settings)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save audio settings to {}: {err}", path.display());
    }
}

fn sync_sfx_volume(settings: Res<AudioSettings>, mut sinks: Query<(&mut AudioSink, &SfxAudio)>) {
    if !settings.is_changed() {
        return;
    }
    let scale = settings.effective_sfx();
    for (mut sink, base) in &mut sinks {
        sink.set_volume(Volume::Linear(base.0 * scale));
    }
}

fn on_settings_click(
    click: On<PointerClick>,
    actions: Query<&SettingsAction>,
    mut commands: Commands,
    mut settings: ResMut<AudioSettings>,
    mut overlay: ResMut<Overlay>,
    fx: Option<Res<Fx>>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    match *action {
        SettingsAction::Step(ch, delta) => {
            settings.step(ch, delta);
        }
        SettingsAction::ToggleMute => {
            settings.muted = !settings.muted;
        }
        SettingsAction::TestSfx => {
            if let Some(fx) = fx {
                let s = fx.sounds.hit_tick.clone();
                play_sfx(&mut commands, &s, 0.85, &settings);
            }
        }
        SettingsAction::ResetDefaults => {
            *settings = AudioSettings::default();
        }
        SettingsAction::Done => {
            save_settings(&settings);
            overlay.settings = false;
        }
    }
}

fn on_slider_press(
    press: On<PointerPress>,
    segments: Query<&SliderSegment>,
    mut settings: ResMut<AudioSettings>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if let Ok(seg) = segments.get(press.entity) {
        settings.set(seg.channel, seg.value);
    }
}

fn on_slider_over(
    over: On<PointerOver>,
    mouse: Res<ButtonInput<MouseButton>>,
    segments: Query<&SliderSegment>,
    mut settings: ResMut<AudioSettings>,
) {
    if !mouse.pressed(MouseButton::Left) {
        return;
    }
    if let Ok(seg) = segments.get(over.entity) {
        settings.set(seg.channel, seg.value);
    }
}

fn show_settings_screen(
    mut commands: Commands,
    overlay: Res<Overlay>,
    settings: Res<AudioSettings>,
    root_q: Query<Entity, With<SettingsRoot>>,
    mut was_open: Local<bool>,
) {
    let open = overlay.settings;
    if !open {
        if *was_open {
            *was_open = false;
            save_settings(&settings);
            for root in &root_q {
                commands.entity(root).despawn();
            }
        }
        return;
    }

    if *was_open && !settings.is_changed() && !root_q.is_empty() {
        return;
    }

    *was_open = true;
    for root in &root_q {
        commands.entity(root).despawn();
    }
    spawn_settings_modal(&mut commands, &settings);
}

fn spawn_settings_modal(commands: &mut Commands, settings: &AudioSettings) {
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
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.65)),
            GlobalZIndex(30),
            SettingsRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(520.0),
                    max_width: Val::Percent(92.0),
                    padding: UiRect::all(Val::Px(28.0)),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: Val::Px(16.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(16.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|card| {
                // Header
                card.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        padding: UiRect::bottom(Val::Px(10.0)),
                        border: UiRect::bottom(Val::Px(1.0)),
                        ..default()
                    },
                    BorderColor::all(BORDER_SUBTLE),
                    Pickable::IGNORE,
                ))
                .with_children(|header| {
                    header.spawn((
                        Text::new("OPTIONS"),
                        TextFont::from_font_size(24.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    header.spawn((
                        Text::new("Audio Volume  •  Click or drag bars to adjust"),
                        TextFont::from_font_size(13.0),
                        TextColor(TEXT_MUTED),
                        Pickable::IGNORE,
                    ));
                });

                // Volume channels
                for channel in AudioChannel::ALL {
                    spawn_volume_row(card, channel, settings);
                }

                // Utility row: Mute / Test SFX / Reset
                card.spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(10.0),
                    margin: UiRect::top(Val::Px(4.0)),
                    ..default()
                })
                .with_children(|row| {
                    let (mute_label, mute_style) = if settings.muted {
                        ("Unmute All", ButtonStyle::DANGER)
                    } else {
                        ("Mute All", ButtonStyle::SECONDARY)
                    };
                    spawn_button(
                        row,
                        mute_label,
                        SettingsAction::ToggleMute,
                        mute_style,
                        40.0,
                        14.0,
                        true,
                    );
                    spawn_button(
                        row,
                        "Test Sound",
                        SettingsAction::TestSfx,
                        ButtonStyle::SECONDARY,
                        40.0,
                        14.0,
                        true,
                    );
                    spawn_button(
                        row,
                        "Reset",
                        SettingsAction::ResetDefaults,
                        ButtonStyle::SECONDARY,
                        40.0,
                        14.0,
                        true,
                    );
                });

                // Done button
                spawn_button(
                    card,
                    "Done",
                    SettingsAction::Done,
                    ButtonStyle::PRIMARY,
                    46.0,
                    17.0,
                    false,
                );
            });
        });
}

fn spawn_volume_row(
    parent: &mut ChildSpawnerCommands,
    channel: AudioChannel,
    settings: &AudioSettings,
) {
    let val = settings.get(channel);
    let pct = (val * 100.0).round() as u32;
    let readout = if settings.muted {
        format!("{pct}% (MUTED)")
    } else {
        format!("{pct}%")
    };

    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|group| {
            // Label + percentage readout
            group
                .spawn((
                    Node {
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|top| {
                    top.spawn((
                        Text::new(channel.label()),
                        TextFont::from_font_size(15.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    top.spawn((
                        Text::new(readout),
                        TextFont::from_font_size(14.0),
                        TextColor(if settings.muted {
                            BTN_DANGER
                        } else {
                            TEXT_STATUS
                        }),
                        Pickable::IGNORE,
                    ));
                });

            // Controls row: [-] [20-segment slider bar] [+]
            group
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|row| {
                    // Minus button
                    row.spawn((
                        Node {
                            width: Val::Px(34.0),
                            height: Val::Px(30.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(7.0)),
                            ..default()
                        },
                        BackgroundColor(ButtonStyle::SECONDARY.normal),
                        ButtonStyle::SECONDARY,
                        SettingsAction::Step(channel, -0.05),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new("-"),
                            TextFont::from_font_size(18.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                        ));
                    });

                    // Slider bar: 21 clickable/draggable notches (0%..=100% in 5% increments)
                    row.spawn((
                        Node {
                            flex_grow: 1.0,
                            height: Val::Px(26.0),
                            padding: UiRect::all(Val::Px(3.0)),
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(2.0),
                            align_items: AlignItems::Stretch,
                            border: UiRect::all(Val::Px(1.0)),
                            border_radius: BorderRadius::all(Val::Px(8.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.92)),
                        BorderColor::all(BORDER_SUBTLE),
                    ))
                    .with_children(|track| {
                        for step in 0..=20 {
                            let step_val = step as f32 / 20.0;
                            let filled = step > 0 && val + 0.001 >= step_val;
                            let color = if step == 0 {
                                if val <= 0.001 {
                                    Color::srgb(0.45, 0.24, 0.28)
                                } else {
                                    Color::srgba(0.22, 0.28, 0.36, 0.5)
                                }
                            } else if filled {
                                if settings.muted {
                                    Color::srgb(0.35, 0.40, 0.48)
                                } else {
                                    BTN_PRIMARY
                                }
                            } else {
                                Color::srgba(0.16, 0.21, 0.29, 0.75)
                            };
                            track.spawn((
                                Node {
                                    flex_grow: 1.0,
                                    height: Val::Percent(100.0),
                                    border_radius: BorderRadius::all(Val::Px(3.0)),
                                    ..default()
                                },
                                BackgroundColor(color),
                                SliderSegment {
                                    channel,
                                    value: step_val,
                                },
                            ));
                        }
                    });

                    // Plus button
                    row.spawn((
                        Node {
                            width: Val::Px(34.0),
                            height: Val::Px(30.0),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(7.0)),
                            ..default()
                        },
                        BackgroundColor(ButtonStyle::SECONDARY.normal),
                        ButtonStyle::SECONDARY,
                        SettingsAction::Step(channel, 0.05),
                    ))
                    .with_children(|btn| {
                        btn.spawn((
                            Text::new("+"),
                            TextFont::from_font_size(18.0),
                            TextColor(TEXT_PRIMARY),
                            Pickable::IGNORE,
                        ));
                    });
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_settings_clamp_and_mute() {
        let mut s = AudioSettings {
            master: 1.5,
            music: -0.2,
            sfx: f32::NAN,
            ui: 0.5,
            muted: false,
        }
        .sanitized();
        assert_eq!(s.master, 1.0);
        assert_eq!(s.music, 0.0);
        assert_eq!(s.sfx, 1.0);
        assert_eq!(s.ui, 0.5);

        s.master = 0.5;
        s.sfx = 0.8;
        assert!((s.effective_sfx() - 0.4).abs() < 1e-5);

        s.muted = true;
        assert_eq!(s.effective_music(), 0.0);
        assert_eq!(s.effective_sfx(), 0.0);
        assert_eq!(s.effective_ui(), 0.0);

        // Adjusting a slider unmutes automatically.
        s.set(AudioChannel::Music, 0.6);
        assert!(!s.muted);
        assert!((s.music - 0.6).abs() < 1e-5);
    }
}

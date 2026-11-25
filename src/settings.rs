//! Persistent game settings (Display, Audio, Crosshair, Mouse & View, Keybinds)
//! and the multi-section Options screen.
//!
//! Opens as a top-bar tab in the Main Menu (keeping the top bar clickable) and
//! as a full-screen overlay from the in-game Escape pause menu.

use crate::{
    AppState, Overlay,
    loadout::CROSSHAIRS,
    menu::{
        BG_CARD, BORDER_SUBTLE, BTN_DANGER, BTN_PRIMARY, ButtonStyle, MenuScreen, MenuState,
        TEXT_MUTED, TEXT_PRIMARY, TEXT_STATUS, TOP_BAR_HEIGHT, spawn_button,
    },
    ui_kit::KIT_YELLOW,
};
use bevy::{
    audio::{AudioSink, AudioSinkPlayback, Volume},
    picking::prelude::*,
    prelude::*,
    window::{MonitorSelection, PrimaryWindow, VideoModeSelection, WindowMode},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsUiState>()
            .insert_resource(load_view_settings())
            .add_observer(on_settings_click)
            .add_observer(on_slider_press)
            .add_observer(on_slider_over)
            .add_systems(
                Update,
                (
                    listen_for_keybind,
                    show_settings_screen,
                    sync_sfx_volume,
                    apply_display_settings,
                    persist_view_settings,
                ),
            );
    }
}

// ---------------------------------------------------------------------------
// Settings data models
// ---------------------------------------------------------------------------

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

/// Window presentation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WindowModeOption {
    #[default]
    Windowed,
    Borderless,
    Fullscreen,
}

impl WindowModeOption {
    const ALL: [Self; 3] = [Self::Windowed, Self::Borderless, Self::Fullscreen];

    pub fn label(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::Borderless => "Borderless Fullscreen",
            Self::Fullscreen => "Fullscreen",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Windowed => "Standard resizable window.",
            Self::Borderless => "Fills the screen with instant Alt+Tab switching.",
            Self::Fullscreen => "Dedicated exclusive fullscreen on the current monitor.",
        }
    }

    pub fn to_window_mode(self) -> WindowMode {
        match self {
            Self::Windowed => WindowMode::Windowed,
            Self::Borderless => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
            Self::Fullscreen => {
                WindowMode::Fullscreen(MonitorSelection::Current, VideoModeSelection::Current)
            }
        }
    }
}

/// Frame-rate caps offered in Display settings (0 = unlimited).
pub const FPS_LIMITS: [u32; 7] = [30, 60, 120, 144, 165, 240, 0];

/// Persisted display settings.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub window_mode: WindowModeOption,
    /// Small frames-per-second readout in the top-right corner.
    pub show_fps: bool,
    /// Sync presentation to the monitor's refresh rate.
    pub vsync: bool,
    /// Frame-rate cap in frames per second; 0 means unlimited.
    pub fps_limit: u32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            window_mode: WindowModeOption::default(),
            show_fps: false,
            vsync: true,
            fps_limit: 0,
        }
    }
}

impl DisplaySettings {
    pub fn sanitized(mut self) -> Self {
        if self.fps_limit != 0 {
            self.fps_limit = self.fps_limit.clamp(30, 1000);
        }
        self
    }
}

fn fps_limit_label(limit: u32) -> String {
    if limit == 0 {
        "Unlimited".to_string()
    } else {
        limit.to_string()
    }
}

/// Persisted crosshair choice (index into [`CROSSHAIRS`]).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CrosshairSettings {
    pub style: u8,
}

impl CrosshairSettings {
    pub fn sanitized(mut self) -> Self {
        self.style = self.style.min(CROSSHAIRS.len() as u8 - 1);
        self
    }
}

/// Persisted mouse-look and camera settings (sensitivity, ADS sensitivity,
/// invert-Y, field of view). Stored in its own `view.json` beside
/// `settings.json` and saved automatically whenever it changes.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    /// Hip-fire mouse sensitivity multiplier (1.0 = the original feel).
    pub sensitivity: f32,
    /// Extra multiplier applied while aiming down sights, on top of the
    /// usual slow-down from the sight's zoom (1.0 = unchanged).
    pub ads_sensitivity: f32,
    /// Moving the mouse up looks down.
    pub invert_y: bool,
    /// Vertical field of view in degrees.
    pub fov_degrees: f32,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            ads_sensitivity: 1.0,
            invert_y: false,
            fov_degrees: crate::world::BASE_FOV.to_degrees().round(),
        }
    }
}

impl ViewSettings {
    pub fn sanitized(mut self) -> Self {
        let defaults = Self::default();
        for slider in ViewSlider::ALL {
            let v = slider.get(&self);
            let v = if v.is_finite() {
                v
            } else {
                slider.get(&defaults)
            };
            slider.set(&mut self, v);
        }
        self
    }

    /// Vertical field of view in radians, for the camera projection.
    pub fn fov_radians(&self) -> f32 {
        self.fov_degrees.to_radians()
    }

    /// Mouse sensitivity multiplier at the given aim-down-sights blend (0..=1).
    pub fn look_multiplier(&self, aim_blend: f32) -> f32 {
        let ads = 1.0 + (self.ads_sensitivity - 1.0) * aim_blend.clamp(0.0, 1.0);
        self.sensitivity * ads
    }

    /// Sign applied to vertical mouse motion.
    pub fn y_sign(&self) -> f32 {
        if self.invert_y { -1.0 } else { 1.0 }
    }
}

/// The adjustable sliders in the Mouse & View section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSlider {
    Sensitivity,
    AdsSensitivity,
    Fov,
}

impl ViewSlider {
    const ALL: [Self; 3] = [Self::Sensitivity, Self::AdsSensitivity, Self::Fov];

    fn label(self) -> &'static str {
        match self {
            Self::Sensitivity => "Mouse Sensitivity",
            Self::AdsSensitivity => "ADS Sensitivity",
            Self::Fov => "Field of View",
        }
    }

    /// (min, max, step) for this slider.
    fn range(self) -> (f32, f32, f32) {
        match self {
            Self::Sensitivity => (0.1, 3.0, 0.05),
            Self::AdsSensitivity => (0.1, 2.0, 0.05),
            Self::Fov => (50.0, 110.0, 1.0),
        }
    }

    fn get(self, s: &ViewSettings) -> f32 {
        match self {
            Self::Sensitivity => s.sensitivity,
            Self::AdsSensitivity => s.ads_sensitivity,
            Self::Fov => s.fov_degrees,
        }
    }

    fn set(self, s: &mut ViewSettings, value: f32) {
        let (min, max, step) = self.range();
        let snapped = ((value.clamp(min, max) - min) / step).round() * step + min;
        let v = snapped.clamp(min, max);
        match self {
            Self::Sensitivity => s.sensitivity = v,
            Self::AdsSensitivity => s.ads_sensitivity = v,
            Self::Fov => s.fov_degrees = v,
        }
    }

    fn step(self, s: &mut ViewSettings, steps: f32) {
        let (_, _, step) = self.range();
        let next = self.get(s) + steps * step;
        self.set(s, next);
    }

    fn readout(self, s: &ViewSettings) -> String {
        match self {
            Self::Sensitivity | Self::AdsSensitivity => format!("{:.2}x", self.get(s)),
            Self::Fov => format!("{:.0} deg", self.get(s)),
        }
    }
}

/// A single keyboard key or mouse button bound to a gameplay action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputBinding {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl InputBinding {
    pub fn pressed(self, keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> bool {
        match self {
            Self::Key(k) => keys.pressed(k),
            Self::Mouse(m) => mouse.pressed(m),
        }
    }

    pub fn just_pressed(
        self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        match self {
            Self::Key(k) => keys.just_pressed(k),
            Self::Mouse(m) => mouse.just_pressed(m),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mouse(MouseButton::Left) => "LMB",
            Self::Mouse(MouseButton::Right) => "RMB",
            Self::Mouse(MouseButton::Middle) => "MMB",
            Self::Mouse(MouseButton::Back) => "Mouse 4",
            Self::Mouse(MouseButton::Forward) => "Mouse 5",
            Self::Mouse(_) => "Mouse",
            Self::Key(k) => key_label(k),
        }
    }

    fn to_id(self) -> String {
        match self {
            Self::Mouse(MouseButton::Left) => "MouseLeft".into(),
            Self::Mouse(MouseButton::Right) => "MouseRight".into(),
            Self::Mouse(MouseButton::Middle) => "MouseMiddle".into(),
            Self::Mouse(MouseButton::Back) => "MouseBack".into(),
            Self::Mouse(MouseButton::Forward) => "MouseForward".into(),
            Self::Mouse(_) => "MouseLeft".into(),
            Self::Key(k) => format!("{k:?}"),
        }
    }

    fn from_id(s: &str) -> Option<Self> {
        match s {
            "MouseLeft" => Some(Self::Mouse(MouseButton::Left)),
            "MouseRight" => Some(Self::Mouse(MouseButton::Right)),
            "MouseMiddle" => Some(Self::Mouse(MouseButton::Middle)),
            "MouseBack" => Some(Self::Mouse(MouseButton::Back)),
            "MouseForward" => Some(Self::Mouse(MouseButton::Forward)),
            other => parse_key_code(other).map(Self::Key),
        }
    }
}

impl Serialize for InputBinding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_id())
    }
}

impl<'de> Deserialize<'de> for InputBinding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_id(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown binding: {s}")))
    }
}

/// Every rebindable gameplay action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyAction {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sprint,
    Crouch,
    Shoot,
    Aim,
    Reload,
    PaintBomb,
    Flashbang,
    Scoreboard,
}

impl KeyAction {
    pub const ALL: [Self; 13] = [
        Self::MoveForward,
        Self::MoveBackward,
        Self::MoveLeft,
        Self::MoveRight,
        Self::Jump,
        Self::Sprint,
        Self::Crouch,
        Self::Shoot,
        Self::Aim,
        Self::Reload,
        Self::PaintBomb,
        Self::Flashbang,
        Self::Scoreboard,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MoveForward => "Move Forward",
            Self::MoveBackward => "Move Backward",
            Self::MoveLeft => "Move Left",
            Self::MoveRight => "Move Right",
            Self::Jump => "Jump",
            Self::Sprint => "Sprint",
            Self::Crouch => "Crouch",
            Self::Shoot => "Shoot",
            Self::Aim => "Aim Down Sights",
            Self::Reload => "Reload",
            Self::PaintBomb => "Paint Bomb",
            Self::Flashbang => "Flashbang",
            Self::Scoreboard => "Scoreboard",
        }
    }
}

/// Persisted keybindings for gameplay controls.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindSettings {
    pub move_forward: InputBinding,
    pub move_backward: InputBinding,
    pub move_left: InputBinding,
    pub move_right: InputBinding,
    pub jump: InputBinding,
    pub sprint: InputBinding,
    pub crouch: InputBinding,
    pub shoot: InputBinding,
    pub aim: InputBinding,
    pub reload: InputBinding,
    pub paint_bomb: InputBinding,
    pub flashbang: InputBinding,
    pub scoreboard: InputBinding,
}

impl Default for KeybindSettings {
    fn default() -> Self {
        Self {
            move_forward: InputBinding::Key(KeyCode::KeyW),
            move_backward: InputBinding::Key(KeyCode::KeyS),
            move_left: InputBinding::Key(KeyCode::KeyA),
            move_right: InputBinding::Key(KeyCode::KeyD),
            jump: InputBinding::Key(KeyCode::Space),
            sprint: InputBinding::Key(KeyCode::ShiftLeft),
            crouch: InputBinding::Key(KeyCode::KeyC),
            shoot: InputBinding::Mouse(MouseButton::Left),
            aim: InputBinding::Mouse(MouseButton::Right),
            reload: InputBinding::Key(KeyCode::KeyR),
            paint_bomb: InputBinding::Key(KeyCode::KeyG),
            flashbang: InputBinding::Key(KeyCode::KeyQ),
            scoreboard: InputBinding::Key(KeyCode::Tab),
        }
    }
}

impl KeybindSettings {
    pub fn get(&self, action: KeyAction) -> InputBinding {
        match action {
            KeyAction::MoveForward => self.move_forward,
            KeyAction::MoveBackward => self.move_backward,
            KeyAction::MoveLeft => self.move_left,
            KeyAction::MoveRight => self.move_right,
            KeyAction::Jump => self.jump,
            KeyAction::Sprint => self.sprint,
            KeyAction::Crouch => self.crouch,
            KeyAction::Shoot => self.shoot,
            KeyAction::Aim => self.aim,
            KeyAction::Reload => self.reload,
            KeyAction::PaintBomb => self.paint_bomb,
            KeyAction::Flashbang => self.flashbang,
            KeyAction::Scoreboard => self.scoreboard,
        }
    }

    pub fn set(&mut self, action: KeyAction, binding: InputBinding) {
        match action {
            KeyAction::MoveForward => self.move_forward = binding,
            KeyAction::MoveBackward => self.move_backward = binding,
            KeyAction::MoveLeft => self.move_left = binding,
            KeyAction::MoveRight => self.move_right = binding,
            KeyAction::Jump => self.jump = binding,
            KeyAction::Sprint => self.sprint = binding,
            KeyAction::Crouch => self.crouch = binding,
            KeyAction::Shoot => self.shoot = binding,
            KeyAction::Aim => self.aim = binding,
            KeyAction::Reload => self.reload = binding,
            KeyAction::PaintBomb => self.paint_bomb = binding,
            KeyAction::Flashbang => self.flashbang = binding,
            KeyAction::Scoreboard => self.scoreboard = binding,
        }
    }

    pub fn pressed(
        &self,
        action: KeyAction,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        self.get(action).pressed(keys, mouse)
    }

    pub fn just_pressed(
        &self,
        action: KeyAction,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        self.get(action).just_pressed(keys, mouse)
    }
}

/// Combined persistent settings stored in `settings.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SavedSettings {
    pub audio: AudioSettings,
    pub display: DisplaySettings,
    pub crosshair: CrosshairSettings,
    pub keybinds: KeybindSettings,
}

impl SavedSettings {
    pub fn sanitized(mut self) -> Self {
        self.audio = self.audio.sanitized();
        self.display = self.display.sanitized();
        self.crosshair = self.crosshair.sanitized();
        self
    }
}

// ---------------------------------------------------------------------------
// Options UI state & components
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Display,
    Audio,
    Crosshair,
    View,
    Keybinds,
}

impl SettingsTab {
    const ALL: [Self; 5] = [
        Self::Display,
        Self::Audio,
        Self::Crosshair,
        Self::View,
        Self::Keybinds,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Display => "Display",
            Self::Audio => "Audio",
            Self::Crosshair => "Crosshair",
            Self::View => "Mouse & View",
            Self::Keybinds => "Keybinds",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Display => "Window mode, frame rate and screen presentation",
            Self::Audio => "Volume levels  •  Click or drag bars to adjust",
            Self::Crosshair => "Choose the aiming reticle shown in-game",
            Self::View => "Look sensitivity and field of view  •  Click or drag bars to adjust",
            Self::Keybinds => "Click a control and press any key or mouse button to rebind",
        }
    }

    fn reset_label(self) -> &'static str {
        match self {
            Self::Display => "Reset Display",
            Self::Audio => "Reset Audio",
            Self::Crosshair => "Reset Crosshair",
            Self::View => "Reset Mouse & View",
            Self::Keybinds => "Reset Keybinds",
        }
    }
}

/// Tracks which Options section is active and whether a keybind is being captured.
#[derive(Resource, Default)]
pub struct SettingsUiState {
    pub tab: SettingsTab,
    pub listening: Option<KeyAction>,
    /// Prevents the click that started rebinding from being immediately bound as LMB.
    skip_capture_frame: bool,
}

impl SettingsUiState {
    pub fn is_listening(&self) -> bool {
        self.listening.is_some()
    }
}

#[derive(Component)]
struct SettingsRoot;

#[derive(Component, Clone, Copy)]
enum SettingsAction {
    SelectTab(SettingsTab),
    SetWindowMode(WindowModeOption),
    ToggleFpsCounter,
    ToggleVsync,
    SetFpsLimit(u32),
    StepAudio(AudioChannel, f32),
    ToggleMute,
    SetCrosshair(u8),
    StepView(ViewSlider, f32),
    ToggleInvertY,
    StartRebind(KeyAction),
    ResetTab(SettingsTab),
    Done,
}

/// Interactive 5%-step segment inside a volume slider bar.
#[derive(Component, Clone, Copy)]
struct SliderSegment {
    channel: AudioChannel,
    value: f32,
}

/// Interactive notch inside a Mouse & View slider bar.
#[derive(Component, Clone, Copy)]
struct ViewSliderSegment {
    slider: ViewSlider,
    value: f32,
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

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

fn view_save_path() -> Option<PathBuf> {
    save_path().map(|p| p.with_file_name("view.json"))
}

pub fn load_view_settings() -> ViewSettings {
    view_save_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<ViewSettings>(&bytes).ok())
        .map(ViewSettings::sanitized)
        .unwrap_or_default()
}

fn save_view_settings(view: &ViewSettings) {
    let Some(path) = view_save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(view)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save view settings to {}: {err}", path.display());
    }
}

/// Writes `view.json` whenever the Mouse & View settings change.
fn persist_view_settings(view: Res<ViewSettings>) {
    if view.is_changed() && !view.is_added() {
        save_view_settings(&view);
    }
}

pub fn load_saved_settings() -> SavedSettings {
    let Some(bytes) = save_path().and_then(|p| std::fs::read(p).ok()) else {
        return SavedSettings::default();
    };
    // Try current combined format first (checking that it actually has a known top-level section
    // rather than being an older AudioSettings-only JSON).
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && val.get("audio").is_none()
        && val.get("master").is_some()
        && let Ok(audio) = serde_json::from_value::<AudioSettings>(val)
    {
        return SavedSettings {
            audio: audio.sanitized(),
            ..SavedSettings::default()
        };
    }
    serde_json::from_slice::<SavedSettings>(&bytes)
        .map(SavedSettings::sanitized)
        .unwrap_or_default()
}

pub fn save_all_settings(
    audio: &AudioSettings,
    display: &DisplaySettings,
    crosshair: &CrosshairSettings,
    keybinds: &KeybindSettings,
) {
    let Some(path) = save_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let combined = SavedSettings {
        audio: *audio,
        display: *display,
        crosshair: *crosshair,
        keybinds: *keybinds,
    };
    if let Ok(json) = serde_json::to_vec_pretty(&combined)
        && let Err(err) = std::fs::write(&path, json)
    {
        warn!("Couldn't save settings to {}: {err}", path.display());
    }
}

// ---------------------------------------------------------------------------
// Systems & observers
// ---------------------------------------------------------------------------

fn sync_sfx_volume(settings: Res<AudioSettings>, mut sinks: Query<(&mut AudioSink, &SfxAudio)>) {
    if !settings.is_changed() {
        return;
    }
    let scale = settings.effective_sfx();
    for (mut sink, base) in &mut sinks {
        sink.set_volume(Volume::Linear(base.0 * scale));
    }
}

fn apply_display_settings(
    display: Res<DisplaySettings>,
    mut window_q: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !display.is_changed() {
        return;
    }
    if let Ok(mut window) = window_q.single_mut() {
        let target = display.window_mode.to_window_mode();
        if window.mode != target {
            window.mode = target;
        }
    }
}

fn listen_for_keybind(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut ui: ResMut<SettingsUiState>,
    mut keybinds: ResMut<KeybindSettings>,
    audio: Res<AudioSettings>,
    display: Res<DisplaySettings>,
    crosshair: Res<CrosshairSettings>,
) {
    let Some(action) = ui.listening else {
        return;
    };
    if ui.skip_capture_frame {
        ui.skip_capture_frame = false;
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        ui.listening = None;
        return;
    }

    // Check keyboard presses.
    for &key in keys.get_just_pressed() {
        if key == KeyCode::Escape {
            continue;
        }
        if parse_key_code(&format!("{key:?}")).is_some() {
            keybinds.set(action, InputBinding::Key(key));
            ui.listening = None;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
            return;
        }
    }

    // Check mouse button presses.
    for btn in [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
    ] {
        if mouse.just_pressed(btn) {
            keybinds.set(action, InputBinding::Mouse(btn));
            ui.listening = None;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn on_settings_click(
    click: On<PointerClick>,
    actions: Query<&SettingsAction>,
    mut ui: ResMut<SettingsUiState>,
    mut audio: ResMut<AudioSettings>,
    mut display: ResMut<DisplaySettings>,
    mut crosshair: ResMut<CrosshairSettings>,
    mut keybinds: ResMut<KeybindSettings>,
    mut view: ResMut<ViewSettings>,
    mut overlay: ResMut<Overlay>,
) {
    if click.button != PointerButton::Primary {
        return;
    }
    // While listening for a keybind, let `listen_for_keybind` handle mouse clicks.
    if ui.listening.is_some() {
        return;
    }
    let Ok(action) = actions.get(click.entity) else {
        return;
    };

    match *action {
        SettingsAction::SelectTab(tab) => {
            ui.tab = tab;
            ui.listening = None;
        }
        SettingsAction::SetWindowMode(mode) => {
            display.window_mode = mode;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::ToggleFpsCounter => {
            display.show_fps = !display.show_fps;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::ToggleVsync => {
            display.vsync = !display.vsync;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::SetFpsLimit(limit) => {
            display.fps_limit = limit;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::StepAudio(ch, delta) => {
            audio.step(ch, delta);
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::ToggleMute => {
            audio.muted = !audio.muted;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::SetCrosshair(style) => {
            crosshair.style = style.min(CROSSHAIRS.len() as u8 - 1);
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::StepView(slider, steps) => slider.step(&mut view, steps),
        SettingsAction::ToggleInvertY => view.invert_y = !view.invert_y,
        SettingsAction::StartRebind(key_action) => {
            ui.listening = Some(key_action);
            ui.skip_capture_frame = true;
        }
        SettingsAction::ResetTab(tab) => {
            match tab {
                SettingsTab::Display => *display = DisplaySettings::default(),
                SettingsTab::Audio => *audio = AudioSettings::default(),
                SettingsTab::Crosshair => *crosshair = CrosshairSettings::default(),
                SettingsTab::View => *view = ViewSettings::default(),
                SettingsTab::Keybinds => {
                    ui.listening = None;
                    *keybinds = KeybindSettings::default();
                }
            }
            save_all_settings(&audio, &display, &crosshair, &keybinds);
        }
        SettingsAction::Done => {
            ui.listening = None;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
            overlay.settings = false;
        }
    }
}

fn on_slider_press(
    press: On<PointerPress>,
    segments: Query<&SliderSegment>,
    view_segments: Query<&ViewSliderSegment>,
    mut audio: ResMut<AudioSettings>,
    mut view: ResMut<ViewSettings>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if let Ok(seg) = segments.get(press.entity) {
        audio.set(seg.channel, seg.value);
    }
    if let Ok(seg) = view_segments.get(press.entity) {
        seg.slider.set(&mut view, seg.value);
    }
}

fn on_slider_over(
    over: On<PointerOver>,
    mouse: Res<ButtonInput<MouseButton>>,
    segments: Query<&SliderSegment>,
    view_segments: Query<&ViewSliderSegment>,
    mut audio: ResMut<AudioSettings>,
    mut view: ResMut<ViewSettings>,
) {
    if !mouse.pressed(MouseButton::Left) {
        return;
    }
    if let Ok(seg) = segments.get(over.entity) {
        audio.set(seg.channel, seg.value);
    }
    if let Ok(seg) = view_segments.get(over.entity) {
        seg.slider.set(&mut view, seg.value);
    }
}

#[allow(clippy::too_many_arguments)]
fn show_settings_screen(
    mut commands: Commands,
    state: Res<State<AppState>>,
    menu: Res<MenuState>,
    overlay: Res<Overlay>,
    mut ui: ResMut<SettingsUiState>,
    audio: Res<AudioSettings>,
    display: Res<DisplaySettings>,
    crosshair: Res<CrosshairSettings>,
    keybinds: Res<KeybindSettings>,
    view: Res<ViewSettings>,
    assets: Res<AssetServer>,
    root_q: Query<Entity, With<SettingsRoot>>,
    mut was_open: Local<bool>,
    mut was_embedded: Local<bool>,
) {
    let embedded = *state.get() == AppState::MainMenu && menu.screen == MenuScreen::Options;
    let open = embedded || overlay.settings;

    if !open {
        if *was_open {
            *was_open = false;
            *was_embedded = false;
            ui.listening = None;
            save_all_settings(&audio, &display, &crosshair, &keybinds);
            for root in &root_q {
                commands.entity(root).despawn();
            }
        }
        return;
    }

    let any_changed = ui.is_changed()
        || audio.is_changed()
        || display.is_changed()
        || crosshair.is_changed()
        || keybinds.is_changed()
        || view.is_changed()
        || *was_embedded != embedded;

    if *was_open && !any_changed && !root_q.is_empty() {
        return;
    }

    *was_open = true;
    *was_embedded = embedded;
    for root in &root_q {
        commands.entity(root).despawn();
    }
    spawn_settings_view(
        &mut commands,
        embedded,
        &ui,
        &audio,
        &display,
        &crosshair,
        &keybinds,
        &view,
        &assets,
    );
}

// ---------------------------------------------------------------------------
// UI construction
// ---------------------------------------------------------------------------

fn style_for(selected: bool) -> ButtonStyle {
    if selected {
        ButtonStyle::PRIMARY
    } else {
        ButtonStyle::SECONDARY
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_settings_view(
    commands: &mut Commands,
    embedded: bool,
    ui: &SettingsUiState,
    audio: &AudioSettings,
    display: &DisplaySettings,
    crosshair: &CrosshairSettings,
    keybinds: &KeybindSettings,
    view: &ViewSettings,
    assets: &AssetServer,
) {
    let root_node = if embedded {
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(TOP_BAR_HEIGHT),
            bottom: Val::Px(0.0),
            padding: UiRect::axes(Val::Px(48.0), Val::Px(28.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Stretch,
            ..default()
        }
    } else {
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(32.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        }
    };

    let root_bg = if embedded {
        Color::NONE
    } else {
        Color::srgba(0.0, 0.0, 0.0, 0.72)
    };
    let z = if embedded { 1 } else { 30 };

    commands
        .spawn((root_node, BackgroundColor(root_bg), GlobalZIndex(z), SettingsRoot))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Percent(100.0),
                    max_width: Val::Px(980.0),
                    height: if embedded {
                        Val::Percent(100.0)
                    } else {
                        Val::Percent(88.0)
                    },
                    max_height: Val::Px(720.0),
                    padding: UiRect::all(Val::Px(24.0)),
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(24.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(16.0)),
                    ..default()
                },
                BackgroundColor(BG_CARD),
                BorderColor::all(BORDER_SUBTLE),
            ))
            .with_children(|container| {
                // Left navigation sidebar
                container
                    .spawn((
                        Node {
                            width: Val::Px(220.0),
                            flex_shrink: 0.0,
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(10.0),
                            padding: UiRect::right(Val::Px(18.0)),
                            border: UiRect::right(Val::Px(1.0)),
                            ..default()
                        },
                        BorderColor::all(BORDER_SUBTLE),
                        Pickable::IGNORE,
                    ))
                    .with_children(|sidebar| {
                        sidebar.spawn((
                            Text::new("OPTIONS"),
                            TextFont::from_font_size(28.0),
                            TextColor(TEXT_PRIMARY),
                            Node {
                                margin: UiRect::bottom(Val::Px(8.0)),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));

                        for tab in SettingsTab::ALL {
                            spawn_button(
                                sidebar,
                                tab.label(),
                                SettingsAction::SelectTab(tab),
                                style_for(ui.tab == tab),
                                44.0,
                                16.0,
                                false,
                            );
                        }

                        // Spacer
                        sidebar.spawn((
                            Node {
                                flex_grow: 1.0,
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));

                        if !embedded {
                            spawn_button(
                                sidebar,
                                "Done",
                                SettingsAction::Done,
                                ButtonStyle::PRIMARY,
                                44.0,
                                17.0,
                                false,
                            );
                        }
                    });

                // Right content pane
                container
                    .spawn((
                        Node {
                            flex_grow: 1.0,
                            flex_basis: Val::Px(0.0),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(16.0),
                            min_height: Val::Px(0.0),
                            ..default()
                        },
                        Pickable::IGNORE,
                    ))
                    .with_children(|pane| {
                        // Section header with per-tab Reset All button
                        pane.spawn((
                            Node {
                                flex_direction: FlexDirection::Row,
                                justify_content: JustifyContent::SpaceBetween,
                                align_items: AlignItems::Center,
                                column_gap: Val::Px(16.0),
                                padding: UiRect::bottom(Val::Px(12.0)),
                                border: UiRect::bottom(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor::all(BORDER_SUBTLE),
                            Pickable::IGNORE,
                        ))
                        .with_children(|header| {
                            header
                                .spawn((
                                    Node {
                                        flex_direction: FlexDirection::Column,
                                        row_gap: Val::Px(4.0),
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ))
                                .with_children(|col| {
                                    col.spawn((
                                        Text::new(ui.tab.label().to_uppercase()),
                                        TextFont::from_font_size(26.0),
                                        TextColor(TEXT_PRIMARY),
                                        Pickable::IGNORE,
                                    ));
                                    col.spawn((
                                        Text::new(ui.tab.subtitle()),
                                        TextFont::from_font_size(13.0),
                                        TextColor(TEXT_MUTED),
                                        Pickable::IGNORE,
                                    ));
                                });

                            header
                                .spawn((
                                    Node {
                                        width: Val::Px(170.0),
                                        flex_direction: FlexDirection::Column,
                                        ..default()
                                    },
                                    Pickable::IGNORE,
                                ))
                                .with_children(|wrap| {
                                    spawn_button(
                                        wrap,
                                        ui.tab.reset_label(),
                                        SettingsAction::ResetTab(ui.tab),
                                        ButtonStyle::SECONDARY,
                                        38.0,
                                        14.0,
                                        false,
                                    );
                                });
                        });

                        // Scrollable section body
                        pane.spawn((
                            Node {
                                flex_grow: 1.0,
                                flex_direction: FlexDirection::Column,
                                row_gap: Val::Px(14.0),
                                overflow: Overflow::scroll_y(),
                                min_height: Val::Px(0.0),
                                padding: UiRect::right(Val::Px(4.0)),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ))
                        .with_children(|body| match ui.tab {
                            SettingsTab::Display => {
                                spawn_display_section(body, display);
                                spawn_frame_rate_section(body, display);
                            }
                            SettingsTab::Audio => spawn_audio_section(body, audio),
                            SettingsTab::Crosshair => {
                                spawn_crosshair_section(body, crosshair, assets)
                            }
                            SettingsTab::View => spawn_view_section(body, view),
                            SettingsTab::Keybinds => {
                                spawn_keybinds_section(body, keybinds, ui.listening)
                            }
                        });
                    });
            });
        });
}

fn spawn_display_section(parent: &mut ChildSpawnerCommands, display: &DisplaySettings) {
    parent.spawn((
        Text::new("WINDOW MODE"),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_MUTED),
        Pickable::IGNORE,
    ));

    for mode in WindowModeOption::ALL {
        let selected = display.window_mode == mode;
        let style = style_for(selected);
        parent
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(14.0)),
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(style.normal),
                style,
                SettingsAction::SetWindowMode(mode),
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(mode.label()),
                    TextFont::from_font_size(16.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));
                row.spawn((
                    Text::new(mode.description()),
                    TextFont::from_font_size(12.0),
                    TextColor(if selected {
                        Color::srgb(0.15, 0.17, 0.20)
                    } else {
                        TEXT_MUTED
                    }),
                    Pickable::IGNORE,
                ));
            });
    }
}

/// A labelled row with an On/Off button on the right.
fn spawn_toggle_row(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    on: bool,
    action: SettingsAction,
) {
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.72)),
            BorderColor::all(BORDER_SUBTLE),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new(label),
                TextFont::from_font_size(15.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
            row.spawn((
                Node {
                    width: Val::Px(120.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|wrap| {
                spawn_button(
                    wrap,
                    if on { "On" } else { "Off" },
                    action,
                    style_for(on),
                    34.0,
                    14.0,
                    false,
                );
            });
        });
}

fn spawn_frame_rate_section(parent: &mut ChildSpawnerCommands, display: &DisplaySettings) {
    parent.spawn((
        Text::new("FRAME RATE"),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_MUTED),
        Node {
            margin: UiRect::top(Val::Px(8.0)),
            ..default()
        },
        Pickable::IGNORE,
    ));
    spawn_toggle_row(
        parent,
        "Show FPS Counter",
        display.show_fps,
        SettingsAction::ToggleFpsCounter,
    );
    spawn_toggle_row(parent, "VSync", display.vsync, SettingsAction::ToggleVsync);

    parent.spawn((
        Text::new("FPS LIMIT"),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_MUTED),
        Pickable::IGNORE,
    ));
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(8.0),
                row_gap: Val::Px(8.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            for limit in FPS_LIMITS {
                row.spawn((
                    Node {
                        width: Val::Px(if limit == 0 { 120.0 } else { 76.0 }),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|wrap| {
                    spawn_button(
                        wrap,
                        &fps_limit_label(limit),
                        SettingsAction::SetFpsLimit(limit),
                        style_for(display.fps_limit == limit),
                        34.0,
                        14.0,
                        false,
                    );
                });
            }
        });
    parent.spawn((
        Text::new(
            "With VSync on, the frame rate also stops at your monitor's refresh rate. \
             Gameplay runs the same at any frame rate.",
        ),
        TextFont::from_font_size(12.0),
        TextColor(TEXT_MUTED),
        Pickable::IGNORE,
    ));
}

fn spawn_audio_section(parent: &mut ChildSpawnerCommands, audio: &AudioSettings) {
    for channel in AudioChannel::ALL {
        spawn_volume_row(parent, channel, audio);
    }

    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(12.0),
                margin: UiRect::top(Val::Px(8.0)),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            let (mute_label, mute_style) = if audio.muted {
                ("Unmute All", ButtonStyle::DANGER)
            } else {
                ("Mute All", ButtonStyle::SECONDARY)
            };
            row.spawn((
                Node {
                    width: Val::Px(200.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|wrap| {
                spawn_button(
                    wrap,
                    mute_label,
                    SettingsAction::ToggleMute,
                    mute_style,
                    42.0,
                    15.0,
                    false,
                );
            });
        });
}

fn spawn_crosshair_section(
    parent: &mut ChildSpawnerCommands,
    crosshair: &CrosshairSettings,
    assets: &AssetServer,
) {
    parent.spawn((
        Text::new("CROSSHAIR STYLE"),
        TextFont::from_font_size(13.0),
        TextColor(TEXT_MUTED),
        Pickable::IGNORE,
    ));

    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: Val::Px(16.0),
                row_gap: Val::Px(16.0),
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            for (i, (name, path)) in CROSSHAIRS.iter().enumerate() {
                let selected = crosshair.style == i as u8;
                let style = style_for(selected);
                row.spawn((
                    Node {
                        width: Val::Px(150.0),
                        padding: UiRect::all(Val::Px(16.0)),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(12.0),
                        border_radius: BorderRadius::all(Val::Px(12.0)),
                        ..default()
                    },
                    BackgroundColor(style.normal),
                    style,
                    SettingsAction::SetCrosshair(i as u8),
                ))
                .with_children(|card| {
                    card.spawn((
                        Node {
                            width: Val::Px(72.0),
                            height: Val::Px(72.0),
                            padding: UiRect::all(Val::Px(14.0)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            border_radius: BorderRadius::all(Val::Px(10.0)),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.85)),
                        Pickable::IGNORE,
                    ))
                    .with_children(|preview| {
                        preview.spawn((
                            ImageNode::new(assets.load(*path)),
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Percent(100.0),
                                ..default()
                            },
                            Pickable::IGNORE,
                        ));
                    });
                    card.spawn((
                        Text::new(*name),
                        TextFont::from_font_size(16.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                });
            }
        });
}

fn spawn_view_section(parent: &mut ChildSpawnerCommands, view: &ViewSettings) {
    for slider in ViewSlider::ALL {
        spawn_view_slider_row(parent, slider, view);
    }

    // Invert-Y toggle row
    parent
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                margin: UiRect::top(Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(10.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.72)),
            BorderColor::all(BORDER_SUBTLE),
            Pickable::IGNORE,
        ))
        .with_children(|row| {
            row.spawn((
                Text::new("Invert Y-Axis"),
                TextFont::from_font_size(15.0),
                TextColor(TEXT_PRIMARY),
                Pickable::IGNORE,
            ));
            row.spawn((
                Node {
                    width: Val::Px(120.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Pickable::IGNORE,
            ))
            .with_children(|wrap| {
                spawn_button(
                    wrap,
                    if view.invert_y { "On" } else { "Off" },
                    SettingsAction::ToggleInvertY,
                    style_for(view.invert_y),
                    34.0,
                    14.0,
                    false,
                );
            });
        });

    parent.spawn((
        Text::new(
            "ADS sensitivity scales your look speed while aiming down sights, on top of the sight's zoom.",
        ),
        TextFont::from_font_size(12.0),
        TextColor(TEXT_MUTED),
        Pickable::IGNORE,
    ));
}

fn spawn_view_slider_row(
    parent: &mut ChildSpawnerCommands,
    slider: ViewSlider,
    view: &ViewSettings,
) {
    let (min, max, _) = slider.range();
    let val = slider.get(view);
    const NOTCHES: u32 = 20;

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
                        Text::new(slider.label()),
                        TextFont::from_font_size(15.0),
                        TextColor(TEXT_PRIMARY),
                        Pickable::IGNORE,
                    ));
                    top.spawn((
                        Text::new(slider.readout(view)),
                        TextFont::from_font_size(14.0),
                        TextColor(TEXT_STATUS),
                        Pickable::IGNORE,
                    ));
                });

            // Controls row: [-] [notched slider bar] [+]
            group
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(8.0),
                    ..default()
                })
                .with_children(|row| {
                    for (label, steps) in [("-", -1.0_f32), ("+", 1.0_f32)] {
                        if steps > 0.0 {
                            spawn_view_track(row, slider, val, min, max, NOTCHES);
                        }
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
                            SettingsAction::StepView(slider, steps),
                        ))
                        .with_children(|btn| {
                            btn.spawn((
                                Text::new(label),
                                TextFont::from_font_size(18.0),
                                TextColor(TEXT_PRIMARY),
                                Pickable::IGNORE,
                            ));
                        });
                    }
                });
        });
}

fn spawn_view_track(
    row: &mut ChildSpawnerCommands,
    slider: ViewSlider,
    val: f32,
    min: f32,
    max: f32,
    notches: u32,
) {
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
        let span = max - min;
        for step in 0..=notches {
            let step_val = min + span * step as f32 / notches as f32;
            let filled = val + span * 0.001 >= step_val;
            let color = if filled {
                BTN_PRIMARY
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
                ViewSliderSegment {
                    slider,
                    value: step_val,
                },
            ));
        }
    });
}

fn spawn_keybinds_section(
    parent: &mut ChildSpawnerCommands,
    keybinds: &KeybindSettings,
    listening: Option<KeyAction>,
) {
    if let Some(action) = listening {
        parent
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(8.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.96, 0.77, 0.17, 0.14)),
                BorderColor::all(KIT_YELLOW),
                Pickable::IGNORE,
            ))
            .with_children(|banner| {
                banner.spawn((
                    Text::new(format!(
                        "Listening for \"{}\"... Press any key or mouse button (Esc to cancel).",
                        action.label()
                    )),
                    TextFont::from_font_size(13.0),
                    TextColor(KIT_YELLOW),
                    Pickable::IGNORE,
                ));
            });
    }

    for action in KeyAction::ALL {
        let is_listening = listening == Some(action);
        let binding_text = if is_listening {
            "Press a key...".to_string()
        } else {
            keybinds.get(action).label().to_string()
        };

        parent
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(10.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.72)),
                BorderColor::all(if is_listening {
                    KIT_YELLOW
                } else {
                    BORDER_SUBTLE
                }),
                Pickable::IGNORE,
            ))
            .with_children(|row| {
                row.spawn((
                    Text::new(action.label()),
                    TextFont::from_font_size(15.0),
                    TextColor(TEXT_PRIMARY),
                    Pickable::IGNORE,
                ));

                row.spawn((
                    Node {
                        width: Val::Px(160.0),
                        flex_direction: FlexDirection::Column,
                        ..default()
                    },
                    Pickable::IGNORE,
                ))
                .with_children(|wrap| {
                    spawn_button(
                        wrap,
                        &binding_text,
                        SettingsAction::StartRebind(action),
                        style_for(is_listening),
                        34.0,
                        14.0,
                        false,
                    );
                });
            });
    }
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
                        SettingsAction::StepAudio(channel, -0.05),
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
                        SettingsAction::StepAudio(channel, 0.05),
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

// ---------------------------------------------------------------------------
// KeyCode <-> string helpers
// ---------------------------------------------------------------------------

fn key_label(key: KeyCode) -> &'static str {
    match key {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Space => "Space",
        KeyCode::Tab => "Tab",
        KeyCode::Enter => "Enter",
        KeyCode::Backspace => "Backspace",
        KeyCode::ShiftLeft => "L-Shift",
        KeyCode::ShiftRight => "R-Shift",
        KeyCode::ControlLeft => "L-Ctrl",
        KeyCode::ControlRight => "R-Ctrl",
        KeyCode::AltLeft => "L-Alt",
        KeyCode::AltRight => "R-Alt",
        KeyCode::CapsLock => "Caps",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowLeft => "Left",
        KeyCode::ArrowRight => "Right",
        KeyCode::Backquote => "`",
        KeyCode::Minus => "-",
        KeyCode::Equal => "=",
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::Backslash => "\\",
        KeyCode::Semicolon => ";",
        KeyCode::Quote => "'",
        KeyCode::Comma => ",",
        KeyCode::Period => ".",
        KeyCode::Slash => "/",
        KeyCode::Insert => "Ins",
        KeyCode::Delete => "Del",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PgUp",
        KeyCode::PageDown => "PgDn",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::Numpad0 => "Num 0",
        KeyCode::Numpad1 => "Num 1",
        KeyCode::Numpad2 => "Num 2",
        KeyCode::Numpad3 => "Num 3",
        KeyCode::Numpad4 => "Num 4",
        KeyCode::Numpad5 => "Num 5",
        KeyCode::Numpad6 => "Num 6",
        KeyCode::Numpad7 => "Num 7",
        KeyCode::Numpad8 => "Num 8",
        KeyCode::Numpad9 => "Num 9",
        _ => "Key",
    }
}

fn parse_key_code(s: &str) -> Option<KeyCode> {
    Some(match s {
        "KeyA" => KeyCode::KeyA,
        "KeyB" => KeyCode::KeyB,
        "KeyC" => KeyCode::KeyC,
        "KeyD" => KeyCode::KeyD,
        "KeyE" => KeyCode::KeyE,
        "KeyF" => KeyCode::KeyF,
        "KeyG" => KeyCode::KeyG,
        "KeyH" => KeyCode::KeyH,
        "KeyI" => KeyCode::KeyI,
        "KeyJ" => KeyCode::KeyJ,
        "KeyK" => KeyCode::KeyK,
        "KeyL" => KeyCode::KeyL,
        "KeyM" => KeyCode::KeyM,
        "KeyN" => KeyCode::KeyN,
        "KeyO" => KeyCode::KeyO,
        "KeyP" => KeyCode::KeyP,
        "KeyQ" => KeyCode::KeyQ,
        "KeyR" => KeyCode::KeyR,
        "KeyS" => KeyCode::KeyS,
        "KeyT" => KeyCode::KeyT,
        "KeyU" => KeyCode::KeyU,
        "KeyV" => KeyCode::KeyV,
        "KeyW" => KeyCode::KeyW,
        "KeyX" => KeyCode::KeyX,
        "KeyY" => KeyCode::KeyY,
        "KeyZ" => KeyCode::KeyZ,
        "Digit0" => KeyCode::Digit0,
        "Digit1" => KeyCode::Digit1,
        "Digit2" => KeyCode::Digit2,
        "Digit3" => KeyCode::Digit3,
        "Digit4" => KeyCode::Digit4,
        "Digit5" => KeyCode::Digit5,
        "Digit6" => KeyCode::Digit6,
        "Digit7" => KeyCode::Digit7,
        "Digit8" => KeyCode::Digit8,
        "Digit9" => KeyCode::Digit9,
        "Space" => KeyCode::Space,
        "Tab" => KeyCode::Tab,
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "ShiftLeft" => KeyCode::ShiftLeft,
        "ShiftRight" => KeyCode::ShiftRight,
        "ControlLeft" => KeyCode::ControlLeft,
        "ControlRight" => KeyCode::ControlRight,
        "AltLeft" => KeyCode::AltLeft,
        "AltRight" => KeyCode::AltRight,
        "CapsLock" => KeyCode::CapsLock,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "Backquote" => KeyCode::Backquote,
        "Minus" => KeyCode::Minus,
        "Equal" => KeyCode::Equal,
        "BracketLeft" => KeyCode::BracketLeft,
        "BracketRight" => KeyCode::BracketRight,
        "Backslash" => KeyCode::Backslash,
        "Semicolon" => KeyCode::Semicolon,
        "Quote" => KeyCode::Quote,
        "Comma" => KeyCode::Comma,
        "Period" => KeyCode::Period,
        "Slash" => KeyCode::Slash,
        "Insert" => KeyCode::Insert,
        "Delete" => KeyCode::Delete,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "F1" => KeyCode::F1,
        "F2" => KeyCode::F2,
        "F3" => KeyCode::F3,
        "F4" => KeyCode::F4,
        "F5" => KeyCode::F5,
        "F6" => KeyCode::F6,
        "F7" => KeyCode::F7,
        "F8" => KeyCode::F8,
        "F9" => KeyCode::F9,
        "F10" => KeyCode::F10,
        "F11" => KeyCode::F11,
        "F12" => KeyCode::F12,
        "Numpad0" => KeyCode::Numpad0,
        "Numpad1" => KeyCode::Numpad1,
        "Numpad2" => KeyCode::Numpad2,
        "Numpad3" => KeyCode::Numpad3,
        "Numpad4" => KeyCode::Numpad4,
        "Numpad5" => KeyCode::Numpad5,
        "Numpad6" => KeyCode::Numpad6,
        "Numpad7" => KeyCode::Numpad7,
        "Numpad8" => KeyCode::Numpad8,
        "Numpad9" => KeyCode::Numpad9,
        _ => return None,
    })
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

    #[test]
    fn view_settings_clamp_snap_and_roundtrip() {
        let s = ViewSettings {
            sensitivity: 9.0,
            ads_sensitivity: f32::NAN,
            invert_y: true,
            fov_degrees: 10.0,
        }
        .sanitized();
        assert_eq!(s.sensitivity, 3.0);
        assert_eq!(s.ads_sensitivity, 1.0);
        assert_eq!(s.fov_degrees, 50.0);
        assert_eq!(s.y_sign(), -1.0);

        let mut s = ViewSettings::default();
        ViewSlider::Sensitivity.step(&mut s, 1.0);
        assert!((s.sensitivity - 1.05).abs() < 1e-4);
        ViewSlider::Fov.set(&mut s, 90.4);
        assert_eq!(s.fov_degrees, 90.0);
        assert!((s.look_multiplier(0.0) - 1.05).abs() < 1e-4);

        let json = serde_json::to_string(&s).unwrap();
        let back: ViewSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn keybinds_roundtrip_json() {
        let mut kb = KeybindSettings::default();
        kb.set(KeyAction::Jump, InputBinding::Mouse(MouseButton::Middle));
        kb.set(KeyAction::Reload, InputBinding::Key(KeyCode::KeyE));
        let json = serde_json::to_string(&kb).unwrap();
        let back: KeybindSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(kb, back);
        assert_eq!(back.get(KeyAction::Jump).label(), "MMB");
        assert_eq!(back.get(KeyAction::Reload).label(), "E");
    }
}

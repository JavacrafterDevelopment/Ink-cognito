//! Frame-rate presentation: the optional FPS counter (top right), VSync and
//! the FPS limiter, all driven by [`DisplaySettings`].
//!
//! Gameplay never depends on the frame rate: everything advances by elapsed
//! time, so capping or uncapping frames only changes smoothness.

use crate::settings::DisplaySettings;
use bevy::{
    picking::prelude::Pickable,
    prelude::*,
    window::{PresentMode, PrimaryWindow},
};
use std::time::{Duration, Instant};

pub struct FrameRatePlugin;

impl Plugin for FrameRatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsSamples>()
            .add_systems(Startup, spawn_fps_counter)
            .add_systems(Update, (apply_vsync, update_fps_counter))
            // Last thing in the frame, so the wait covers all of its work.
            .add_systems(Last, limit_frame_rate);
    }
}

/// How often the counter's number refreshes (seconds).
const FPS_REFRESH: f32 = 0.5;

#[derive(Component)]
struct FpsCounter;

/// Frames counted since the readout last changed.
#[derive(Resource, Default)]
struct FpsSamples {
    frames: u32,
    elapsed: f32,
}

fn spawn_fps_counter(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(12.0),
        TextColor(Color::srgb(0.55, 1.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(4.0),
            top: Val::Px(1.0),
            padding: UiRect::axes(Val::Px(5.0), Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        GlobalZIndex(1000),
        Pickable::IGNORE,
        Visibility::Hidden,
        FpsCounter,
    ));
}

fn update_fps_counter(
    time: Res<Time<Real>>,
    display: Res<DisplaySettings>,
    mut samples: ResMut<FpsSamples>,
    mut counter: Query<(&mut Text, &mut Visibility), With<FpsCounter>>,
) {
    let Ok((mut text, mut vis)) = counter.single_mut() else {
        return;
    };
    if !display.show_fps {
        *vis = Visibility::Hidden;
        samples.frames = 0;
        samples.elapsed = 0.0;
        return;
    }
    *vis = Visibility::Visible;
    samples.frames += 1;
    samples.elapsed += time.delta_secs();
    if samples.elapsed >= FPS_REFRESH || text.0.is_empty() {
        let fps = if samples.elapsed > 0.0 {
            samples.frames as f32 / samples.elapsed
        } else {
            0.0
        };
        text.0 = format!("{:.0} FPS", fps);
        samples.frames = 0;
        samples.elapsed = 0.0;
    }
}

fn apply_vsync(
    display: Res<DisplaySettings>,
    mut window_q: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !display.is_changed() {
        return;
    }
    let Ok(mut window) = window_q.single_mut() else {
        return;
    };
    let target = if display.vsync {
        PresentMode::AutoVsync
    } else {
        PresentMode::AutoNoVsync
    };
    if window.present_mode != target {
        window.present_mode = target;
    }
}

/// Holds each frame until its slot under the FPS cap comes up. Slots are kept
/// on a fixed schedule (not "now + frame time"), so the average lands on the
/// cap exactly instead of drifting low.
fn limit_frame_rate(display: Res<DisplaySettings>, mut next: Local<Option<Instant>>) {
    if display.fps_limit == 0 {
        *next = None;
        return;
    }
    let frame = Duration::from_secs_f64(1.0 / display.fps_limit as f64);
    let now = Instant::now();
    let Some(deadline) = *next else {
        *next = Some(now + frame);
        return;
    };
    if now >= deadline {
        // Running behind the cap: no waiting. If we're over a frame late,
        // restart the schedule rather than rushing frames to catch up.
        *next = Some(if now - deadline > frame {
            now + frame
        } else {
            deadline + frame
        });
        return;
    }
    // Sleep most of the way (OS sleeps can overshoot by about a millisecond),
    // then spin for the rest for an even frame pace.
    let left = deadline - now;
    let margin = Duration::from_micros(1500);
    if left > margin {
        std::thread::sleep(left - margin);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
    *next = Some(deadline + frame);
}

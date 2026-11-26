//! 3D spatial audio engine for Ink-cognito.
//!
//! Why this exists instead of Bevy/rodio's built-in `PlaybackSettings::spatial`:
//! 1. `rodio` 0.22's `Spatial::set_positions` computes `(left_dist - right_dist)`
//!    with the wrong sign, making sounds to your left play louder in your right
//!    earbud and vice versa at normal distances.
//! 2. `rodio`'s `ChannelVolume::next` discards the channel-count division when
//!    downmixing stereo clips to mono, making stereo `.flac` assets 2x too loud.
//! 3. Built-in spatial audio uses a single global inverse-square curve with no
//!    per-sound distance profile, no head-shadow / ITD / rear cues, and no wall
//!    occlusion.
//!
//! Here, every world sound is decoded once into a cached mono PCM buffer and
//! played through a custom stereo [`SpatialClip`] (`AddAudioSource` + `Decodable`).
//! On every frame we update lock-free atomic DSP parameters from the camera's
//! `GlobalTransform`, `Colliders` occlusion raycasts, and `AudioSettings`:
//! - Constant-power + head-shadow ILD panning (`-X` = left ear, `+X` = right ear)
//! - Micro-ITD interaural time delay (~0.62 ms across the head)
//! - Ear-dependent head-shadow low-pass filter, rear-hemisphere warmth, distance
//!   air absorption, and bunker/wall occlusion muffling
//! - Per-class distance rolloff (`Footstep`, `Gunshot`, `SuppressedShot`,
//!   `WaterShot`, `Impact`, `GrenadeBounce`, `Explosion`)
//! - Smooth per-sample parameter slewing so rapid head turns never click

use crate::{
    AppState,
    arena::Colliders,
    settings::AudioSettings,
    world::MainCamera,
};
use bevy::{
    audio::{
        AddAudioSource, AudioPlayer, AudioSource, ChannelCount, Decodable, PlaybackSettings,
        Sample, SampleRate, Source,
    },
    prelude::*,
    transform::TransformSystems,
};
use std::{
    collections::HashMap,
    f32::consts::{FRAC_PI_2, TAU},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

/// Maximum concurrent 3D spatial voices before lower-priority distant ones yield.
const MAX_SPATIAL_VOICES: usize = 36;
/// Ring buffer capacity (power of two) for micro-ITD delay (~1.3 ms at 48 kHz).
const DELAY_RING_LEN: usize = 64;
const DELAY_RING_MASK: usize = DELAY_RING_LEN - 1;
/// Max interaural time difference across a human head (seconds).
const MAX_ITD_SECS: f32 = 0.00062;
/// Minimum crossfeed floor on the far ear at 90 degrees so earbud panning feels
/// grounded and natural while retaining ~16 dB of unmistakable L/R separation.
const FAR_EAR_FLOOR: f32 = 0.16;
/// Distance within which panning smoothly narrows to center so emitters right
/// on top of the camera don't snap harshly between ears.
const NEAR_FIELD_PAN_DIST: f32 = 1.1;

pub struct SpatialAudioPlugin;

impl Plugin for SpatialAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_audio_source::<SpatialClip>()
            .init_resource::<MonoAudioCache>()
            .add_systems(
                PostUpdate,
                (dispatch_spatial_sounds, update_active_spatial_sounds)
                    .chain()
                    .after(TransformSystems::Propagate),
            );
    }
}

/// Acoustic distance, occlusion and priority profile for a class of world sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundProfile {
    /// Player footsteps: crisp up close, audible around corners, quiet across the map.
    Footstep,
    /// Unsuppressed paintball shots: punchy and readable across the whole field.
    Gunshot,
    /// Suppressed barrels: tight, stealthy range.
    SuppressedShot,
    /// Water blaster shots: moderate range.
    WaterShot,
    /// Paint splats, bunker hits, player hits, water splashes.
    Impact,
    /// Grenade throws and bounces.
    GrenadeBounce,
    /// Paint bombs, flashbangs, Ultra Bangs.
    Explosion,
    /// Subtle flyby air-whiz when a shot passes near the camera.
    Flyby,
}

impl SoundProfile {
    /// `(ref_dist, max_dist, rolloff, occlusion_gain, occlusion_cutoff_hz, priority)`.
    pub fn tuning(self) -> ProfileTuning {
        match self {
            SoundProfile::Footstep => ProfileTuning {
                ref_dist: 2.8,
                max_dist: 26.0,
                rolloff: 1.30,
                occlusion_gain: 0.56,
                occlusion_cutoff_hz: 1500.0,
                priority: 3,
            },
            SoundProfile::Gunshot => ProfileTuning {
                ref_dist: 6.5,
                max_dist: 90.0,
                rolloff: 0.85,
                occlusion_gain: 0.70,
                occlusion_cutoff_hz: 2100.0,
                priority: 4,
            },
            SoundProfile::SuppressedShot => ProfileTuning {
                ref_dist: 3.0,
                max_dist: 30.0,
                rolloff: 1.35,
                occlusion_gain: 0.52,
                occlusion_cutoff_hz: 1400.0,
                priority: 3,
            },
            SoundProfile::WaterShot => ProfileTuning {
                ref_dist: 4.0,
                max_dist: 36.0,
                rolloff: 1.15,
                occlusion_gain: 0.58,
                occlusion_cutoff_hz: 1700.0,
                priority: 3,
            },
            SoundProfile::Impact => ProfileTuning {
                ref_dist: 3.5,
                max_dist: 40.0,
                rolloff: 1.15,
                occlusion_gain: 0.55,
                occlusion_cutoff_hz: 1600.0,
                priority: 1,
            },
            SoundProfile::GrenadeBounce => ProfileTuning {
                ref_dist: 3.5,
                max_dist: 32.0,
                rolloff: 1.20,
                occlusion_gain: 0.55,
                occlusion_cutoff_hz: 1500.0,
                priority: 2,
            },
            SoundProfile::Explosion => ProfileTuning {
                ref_dist: 11.0,
                max_dist: 120.0,
                rolloff: 0.72,
                occlusion_gain: 0.75,
                occlusion_cutoff_hz: 2300.0,
                priority: 5,
            },
            SoundProfile::Flyby => ProfileTuning {
                ref_dist: 1.8,
                max_dist: 8.0,
                rolloff: 1.4,
                occlusion_gain: 0.6,
                occlusion_cutoff_hz: 2400.0,
                priority: 2,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProfileTuning {
    pub ref_dist: f32,
    pub max_dist: f32,
    pub rolloff: f32,
    pub occlusion_gain: f32,
    pub occlusion_cutoff_hz: f32,
    pub priority: u8,
}

/// Where a spatial clip plays from.
#[derive(Debug, Clone, Copy)]
pub enum SpatialEmitter {
    /// Fixed point in world space (a shot muzzle, footstep, splat, explosion).
    World(Vec3),
    /// Tracks a live entity's `GlobalTransform` (e.g. a rolling Ultra Bang),
    /// falling back to the last known world position if the entity despawns.
    Follow { entity: Entity, fallback: Vec3 },
    /// Head-relative stereo sound (e.g. local player's own gun slightly to the
    /// right when hipfiring, centered when ADS). `pan` is in `-1.0..=1.0`.
    HeadRelative { pan: f32 },
}

/// Spawn this component to play a 3D spatial (or pitched stereo) sound effect.
/// `dispatch_spatial_sounds` converts it into a playing [`SpatialClip`] on the
/// same frame once the underlying [`AudioSource`] asset is loaded.
#[derive(Component, Clone)]
pub struct SpatialSoundRequest {
    pub source: Handle<AudioSource>,
    pub base_volume: f32,
    pub pitch: f32,
    pub profile: SoundProfile,
    pub emitter: SpatialEmitter,
    /// Drops the request if the asset still hasn't loaded after a few frames.
    pub retries: u8,
}

/// Spawns a 3D spatial world sound at `at`.
pub fn spawn_spatial_sfx(
    commands: &mut Commands,
    source: Handle<AudioSource>,
    base_volume: f32,
    at: Vec3,
    profile: SoundProfile,
    pitch: f32,
) {
    if base_volume <= 0.005 {
        return;
    }
    commands.spawn((
        SpatialSoundRequest {
            source,
            base_volume,
            pitch: pitch.clamp(0.4, 2.2),
            profile,
            emitter: SpatialEmitter::World(at),
            retries: 0,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// Spawns a 3D spatial sound that follows `entity` as it moves (e.g. Ultra Bang).
pub fn spawn_following_sfx(
    commands: &mut Commands,
    source: Handle<AudioSource>,
    base_volume: f32,
    entity: Entity,
    fallback: Vec3,
    profile: SoundProfile,
    pitch: f32,
) {
    if base_volume <= 0.005 {
        return;
    }
    commands.spawn((
        SpatialSoundRequest {
            source,
            base_volume,
            pitch: pitch.clamp(0.4, 2.2),
            profile,
            emitter: SpatialEmitter::Follow { entity, fallback },
            retries: 0,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// Spawns a head-relative stereo SFX (e.g. local gunshots, own footsteps) with
/// proper stereo downmix, pitch variation, subtle panning, and live volume sync.
pub fn spawn_local_sfx(
    commands: &mut Commands,
    source: Handle<AudioSource>,
    base_volume: f32,
    pan: f32,
    pitch: f32,
) {
    if base_volume <= 0.005 {
        return;
    }
    commands.spawn((
        SpatialSoundRequest {
            source,
            base_volume,
            pitch: pitch.clamp(0.4, 2.2),
            profile: SoundProfile::Gunshot,
            emitter: SpatialEmitter::HeadRelative {
                pan: pan.clamp(-1.0, 1.0),
            },
            retries: 0,
        },
        DespawnOnExit(AppState::InGame),
    ));
}

/// Decoded mono PCM audio cached per `AudioSource` asset so every shot, step,
/// and explosion starts with zero decode overhead and clean stereo downmixing.
#[derive(Debug, Clone)]
pub struct MonoBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: SampleRate,
}

impl MonoBuffer {
    fn from_source(source: &AudioSource) -> Self {
        let decoder = source.decoder();
        let channels = decoder.channels().get().max(1) as usize;
        let sample_rate = decoder.sample_rate();
        let mut samples = Vec::new();
        if channels == 1 {
            samples.extend(decoder);
        } else {
            let inv = 1.0 / channels as f32;
            let mut iter = decoder;
            while let Some(first) = iter.next() {
                let mut sum = first;
                for _ in 1..channels {
                    sum += iter.next().unwrap_or(0.0);
                }
                samples.push(sum * inv);
            }
        }
        Self {
            samples,
            sample_rate,
        }
    }
}

#[derive(Resource, Default)]
struct MonoAudioCache {
    buffers: HashMap<AssetId<AudioSource>, Arc<MonoBuffer>>,
}

impl MonoAudioCache {
    fn get_or_decode(
        &mut self,
        handle: &Handle<AudioSource>,
        assets: &Assets<AudioSource>,
    ) -> Option<Arc<MonoBuffer>> {
        let id = handle.id();
        if let Some(buf) = self.buffers.get(&id) {
            return Some(buf.clone());
        }
        let source = assets.get(id)?;
        let buf = Arc::new(MonoBuffer::from_source(source));
        self.buffers.insert(id, buf.clone());
        Some(buf)
    }
}

/// Computed per-ear DSP parameters for one spatial voice at a point in time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialSnapshot {
    pub left_gain: f32,
    pub right_gain: f32,
    /// Interaural delay in samples: negative means left ear leads (source on
    /// left, right ear delayed); positive means right ear leads (source on
    /// right, left ear delayed).
    pub itd_samples: f32,
    /// One-pole low-pass alpha (0..=1; 1.0 = bypass/bright, smaller = darker).
    pub left_lpf: f32,
    pub right_lpf: f32,
    /// Combined audible weight for voice prioritization.
    pub loudness_rank: f32,
}

/// Lock-free atomic bridge from Bevy's frame update to rodio's audio thread.
#[derive(Debug)]
pub struct AtomicSpatialParams {
    left_gain: AtomicU32,
    right_gain: AtomicU32,
    itd_samples: AtomicU32,
    left_lpf: AtomicU32,
    right_lpf: AtomicU32,
}

impl AtomicSpatialParams {
    pub fn new(snap: SpatialSnapshot) -> Self {
        Self {
            left_gain: AtomicU32::new(snap.left_gain.to_bits()),
            right_gain: AtomicU32::new(snap.right_gain.to_bits()),
            itd_samples: AtomicU32::new(snap.itd_samples.to_bits()),
            left_lpf: AtomicU32::new(snap.left_lpf.to_bits()),
            right_lpf: AtomicU32::new(snap.right_lpf.to_bits()),
        }
    }

    pub fn store(&self, snap: SpatialSnapshot) {
        self.left_gain
            .store(snap.left_gain.to_bits(), Ordering::Relaxed);
        self.right_gain
            .store(snap.right_gain.to_bits(), Ordering::Relaxed);
        self.itd_samples
            .store(snap.itd_samples.to_bits(), Ordering::Relaxed);
        self.left_lpf
            .store(snap.left_lpf.to_bits(), Ordering::Relaxed);
        self.right_lpf
            .store(snap.right_lpf.to_bits(), Ordering::Relaxed);
    }

    pub fn load(&self) -> SpatialSnapshot {
        let left_gain = f32::from_bits(self.left_gain.load(Ordering::Relaxed));
        let right_gain = f32::from_bits(self.right_gain.load(Ordering::Relaxed));
        SpatialSnapshot {
            left_gain,
            right_gain,
            itd_samples: f32::from_bits(self.itd_samples.load(Ordering::Relaxed)),
            left_lpf: f32::from_bits(self.left_lpf.load(Ordering::Relaxed)),
            right_lpf: f32::from_bits(self.right_lpf.load(Ordering::Relaxed)),
            loudness_rank: left_gain.max(right_gain),
        }
    }
}

/// Custom Bevy audio asset for 3D spatialized sound playback.
#[derive(Asset, Clone, TypePath)]
pub struct SpatialClip {
    pub buffer: Arc<MonoBuffer>,
    pub pitch: f32,
    pub params: Arc<AtomicSpatialParams>,
}

impl Decodable for SpatialClip {
    type Decoder = SpatialDecoder;

    fn decoder(&self) -> Self::Decoder {
        SpatialDecoder::new(self.buffer.clone(), self.pitch, self.params.clone())
    }
}

/// Real-time stereo spatializer implementing `rodio::Source`.
///
/// Outputs interleaved 2-channel stereo (`channel 0 = LEFT`, `channel 1 = RIGHT`)
/// with fractional-sample pitch resampling, micro-ITD delay line, per-ear
/// one-pole low-pass filtering, per-sample parameter slewing, and soft limiting.
pub struct SpatialDecoder {
    buffer: Arc<MonoBuffer>,
    pitch: f64,
    cursor: f64,
    tail_frames: usize,
    params: Arc<AtomicSpatialParams>,
    ring: [f32; DELAY_RING_LEN],
    ring_write: usize,
    // Smoothed live parameters (prevents clicks when camera rotates quickly).
    cur_left_gain: f32,
    cur_right_gain: f32,
    cur_itd: f32,
    cur_left_lpf: f32,
    cur_right_lpf: f32,
    // One-pole low-pass state per ear.
    lp_left: f32,
    lp_right: f32,
    // Interleaved stereo state.
    emit_right: bool,
    pending_right: f32,
    slew: f32,
}

impl SpatialDecoder {
    pub fn new(buffer: Arc<MonoBuffer>, pitch: f32, params: Arc<AtomicSpatialParams>) -> Self {
        let initial = params.load();
        let sr = buffer.sample_rate.get() as f32;
        // ~3 ms smoothing time constant for zipper-free real-time head tracking.
        let slew = 1.0 - (-1.0 / (0.003 * sr)).exp();
        let max_itd = (MAX_ITD_SECS * sr).ceil() as usize;
        Self {
            buffer,
            pitch: (pitch as f64).clamp(0.4, 2.2),
            cursor: 0.0,
            tail_frames: max_itd + 2,
            params,
            ring: [0.0; DELAY_RING_LEN],
            ring_write: 0,
            cur_left_gain: initial.left_gain,
            cur_right_gain: initial.right_gain,
            cur_itd: initial.itd_samples,
            cur_left_lpf: initial.left_lpf,
            cur_right_lpf: initial.right_lpf,
            lp_left: 0.0,
            lp_right: 0.0,
            emit_right: false,
            pending_right: 0.0,
            slew,
        }
    }

    #[inline]
    fn read_delayed(&self, delay_samples: f32) -> f32 {
        let clamped = delay_samples.clamp(0.0, (DELAY_RING_LEN - 2) as f32);
        let d0 = clamped.floor() as usize;
        let frac = clamped - d0 as f32;
        let idx0 = self.ring_write.wrapping_sub(d0) & DELAY_RING_MASK;
        let idx1 = self.ring_write.wrapping_sub(d0 + 1) & DELAY_RING_MASK;
        self.ring[idx0] + (self.ring[idx1] - self.ring[idx0]) * frac
    }

    #[inline]
    fn sample_source(&self, pos: f64) -> f32 {
        let samples = &self.buffer.samples;
        let i0 = pos.floor() as usize;
        if i0 >= samples.len() {
            return 0.0;
        }
        let s0 = samples[i0];
        let s1 = samples.get(i0 + 1).copied().unwrap_or(0.0);
        let frac = (pos - i0 as f64) as f32;
        s0 + (s1 - s0) * frac
    }
}

#[inline]
fn soft_limit(x: f32) -> f32 {
    if x.abs() <= 0.92 {
        x
    } else {
        let sign = x.signum();
        let excess = x.abs() - 0.92;
        sign * (0.92 + 0.08 * (excess / 0.08).tanh())
    }
}

impl Iterator for SpatialDecoder {
    type Item = Sample;

    fn next(&mut self) -> Option<Self::Item> {
        if self.emit_right {
            self.emit_right = false;
            return Some(self.pending_right);
        }

        let len = self.buffer.samples.len();
        let mono = if (self.cursor as usize) < len {
            let s = self.sample_source(self.cursor);
            self.cursor += self.pitch;
            s
        } else if self.tail_frames > 0 {
            self.tail_frames -= 1;
            0.0
        } else {
            return None;
        };

        let target = self.params.load();
        let k = self.slew;
        self.cur_left_gain += (target.left_gain - self.cur_left_gain) * k;
        self.cur_right_gain += (target.right_gain - self.cur_right_gain) * k;
        self.cur_itd += (target.itd_samples - self.cur_itd) * k;
        self.cur_left_lpf += (target.left_lpf - self.cur_left_lpf) * k;
        self.cur_right_lpf += (target.right_lpf - self.cur_right_lpf) * k;

        self.ring_write = (self.ring_write + 1) & DELAY_RING_MASK;
        self.ring[self.ring_write] = mono;

        // itd < 0 => source on LEFT => left ear has 0 delay, right ear delayed by |itd|.
        // itd > 0 => source on RIGHT => right ear has 0 delay, left ear delayed by itd.
        let left_delay = self.cur_itd.max(0.0);
        let right_delay = (-self.cur_itd).max(0.0);

        let raw_l = self.read_delayed(left_delay);
        let raw_r = self.read_delayed(right_delay);

        self.lp_left += self.cur_left_lpf * (raw_l - self.lp_left);
        self.lp_right += self.cur_right_lpf * (raw_r - self.lp_right);

        let out_l = soft_limit(self.lp_left * self.cur_left_gain);
        let out_r = soft_limit(self.lp_right * self.cur_right_gain);

        self.pending_right = out_r;
        self.emit_right = true;
        Some(out_l)
    }
}

impl Source for SpatialDecoder {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).unwrap()
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.buffer.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        let frames = (self.buffer.samples.len() as f64 / self.pitch).ceil() as u64;
        let secs = frames as f64 / self.buffer.sample_rate.get() as f64;
        Some(Duration::from_secs_f64(secs))
    }
}

/// One-pole low-pass smoothing coefficient `alpha` for cutoff `cutoff_hz`.
fn lpf_alpha(cutoff_hz: f32, sample_rate: f32) -> f32 {
    if cutoff_hz >= sample_rate * 0.44 {
        return 1.0;
    }
    let rc = 1.0 / (TAU * cutoff_hz.max(120.0));
    let dt = 1.0 / sample_rate.max(8000.0);
    (dt / (rc + dt)).clamp(0.01, 1.0)
}

/// Smooth inverse-distance rolloff with clean fade to zero at `max_dist`.
pub fn distance_attenuation(dist: f32, tuning: ProfileTuning) -> f32 {
    if dist >= tuning.max_dist {
        return 0.0;
    }
    let d = dist.max(tuning.ref_dist);
    let inv = tuning.ref_dist / (tuning.ref_dist + tuning.rolloff * (d - tuning.ref_dist));
    // Smooth cosine window over the last 30% of max_dist so sounds fade cleanly
    // to silence rather than cutting off abruptly.
    let fade_start = tuning.max_dist * 0.70;
    let edge = if dist > fade_start {
        let t = ((dist - fade_start) / (tuning.max_dist - fade_start)).clamp(0.0, 1.0);
        0.5 * (1.0 + (t * std::f32::consts::PI).cos())
    } else {
        1.0
    };
    (inv * edge).clamp(0.0, 1.0)
}

/// Checks if a straight line from the listener's eye to the sound source passes
/// through a solid bunker/wall (ignoring hits right next to the source or eye).
fn is_occluded(colliders: &Colliders, listener_pos: Vec3, source_pos: Vec3) -> bool {
    let delta = source_pos - listener_pos;
    let dist = delta.length();
    if dist < 1.0 {
        return false;
    }
    let dir = delta / dist;
    // Pull both endpoints inward slightly so footsteps on the floor or splats on
    // a bunker face don't self-occlude against the surface they sit on.
    let from = listener_pos + dir * 0.25;
    let to = source_pos - dir * 0.35 + Vec3::Y * 0.12;
    if from.distance_squared(to) < 0.09 {
        return false;
    }
    colliders.segment_hit(from, to).is_some()
}

/// Computes exact per-ear gain, ITD, and low-pass cutoffs for a sound source
/// relative to a camera listener transform.
///
/// Bevy right-handed camera convention:
/// - `-Z` = forward, `+Z` = behind
/// - `+X` = right ear, `-X` = left ear
/// - `+Y` = up
#[allow(clippy::too_many_arguments)]
pub fn compute_spatial_params(
    listener_pos: Vec3,
    listener_rot: Quat,
    source_pos: Vec3,
    profile: SoundProfile,
    base_volume: f32,
    sfx_scale: f32,
    occluded: bool,
    sample_rate: f32,
) -> SpatialSnapshot {
    let tuning = profile.tuning();
    let to_source = source_pos - listener_pos;
    let dist = to_source.length();
    let dist_gain = distance_attenuation(dist, tuning);
    let master_gain = base_volume * sfx_scale * dist_gain;

    if master_gain <= 0.0005 {
        return SpatialSnapshot {
            left_gain: 0.0,
            right_gain: 0.0,
            itd_samples: 0.0,
            left_lpf: 1.0,
            right_lpf: 1.0,
            loudness_rank: 0.0,
        };
    }

    let local_dir = if dist > 1e-4 {
        listener_rot.inverse() * (to_source / dist)
    } else {
        Vec3::NEG_Z
    };

    // Pan smoothly narrows to 0 when the source is right inside the head.
    let near_blend = (dist / NEAR_FIELD_PAN_DIST).clamp(0.0, 1.0);
    // +1.0 = full right ear, -1.0 = full left ear, 0.0 = center.
    let pan = (local_dir.x * near_blend).clamp(-1.0, 1.0);

    // Constant-power stereo pan with natural acoustic crossfeed floor.
    let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
    let raw_left = angle.cos();
    let raw_right = angle.sin();
    let left_pan = FAR_EAR_FLOOR + (1.0 - FAR_EAR_FLOOR) * raw_left;
    let right_pan = FAR_EAR_FLOOR + (1.0 - FAR_EAR_FLOOR) * raw_right;

    // Subtle front/back cue: sounds behind the listener (+Z in camera space) are
    // slightly attenuated and warmer so players can tell front from behind.
    let rear = local_dir.z.max(0.0) * near_blend;
    let rear_gain = 1.0 - 0.14 * rear;

    // Wall/bunker occlusion reduces level and muffles high frequencies.
    let occ_gain = if occluded { tuning.occlusion_gain } else { 1.0 };

    let final_gain = master_gain * rear_gain * occ_gain;

    // Interaural time delay (Woodworth approximation): positive when on right.
    let itd_samples = pan * MAX_ITD_SECS * sample_rate;

    // Base cutoff from distance air absorption + rear pinna shading + occlusion.
    let dist_frac = (dist / tuning.max_dist).clamp(0.0, 1.0);
    let air_cutoff = 20_000.0 * (1.0 - 0.78 * dist_frac * dist_frac);
    let rear_cutoff = 20_000.0 - 14_000.0 * rear;
    let mut base_cutoff = air_cutoff.min(rear_cutoff);
    if occluded {
        base_cutoff = base_cutoff.min(tuning.occlusion_cutoff_hz);
    }

    // Ear-specific head shadow: the ear facing away from the sound loses highs.
    let left_shadow = if pan > 0.0 {
        20_000.0 - 16_200.0 * pan
    } else {
        20_000.0
    };
    let right_shadow = if pan < 0.0 {
        20_000.0 - 16_200.0 * (-pan)
    } else {
        20_000.0
    };

    let left_lpf = lpf_alpha(base_cutoff.min(left_shadow), sample_rate);
    let right_lpf = lpf_alpha(base_cutoff.min(right_shadow), sample_rate);

    let left_gain = final_gain * left_pan;
    let right_gain = final_gain * right_pan;

    SpatialSnapshot {
        left_gain,
        right_gain,
        itd_samples,
        left_lpf,
        right_lpf,
        loudness_rank: left_gain.max(right_gain),
    }
}

/// Computes head-relative stereo parameters (used for local weapon & own steps).
pub fn compute_head_relative_params(
    pan: f32,
    base_volume: f32,
    sfx_scale: f32,
) -> SpatialSnapshot {
    let pan = pan.clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * 0.5 * FRAC_PI_2;
    // Normalize so pan = 0 gives gain = 1.0 in both ears (matching 2D playback).
    let norm = std::f32::consts::SQRT_2;
    let gain = base_volume * sfx_scale;
    let left_gain = gain * angle.cos() * norm;
    let right_gain = gain * angle.sin() * norm;
    SpatialSnapshot {
        left_gain,
        right_gain,
        itd_samples: 0.0,
        left_lpf: 1.0,
        right_lpf: 1.0,
        loudness_rank: left_gain.max(right_gain),
    }
}

/// Active playing spatial sound tracked every frame for real-time head tracking
/// and live `AudioSettings` volume updates.
#[derive(Component)]
struct ActiveSpatialSound {
    params: Arc<AtomicSpatialParams>,
    emitter: SpatialEmitter,
    profile: SoundProfile,
    base_volume: f32,
    sample_rate: f32,
    priority: u8,
}

fn listener_pose(camera_q: &Query<&GlobalTransform, With<MainCamera>>) -> (Vec3, Quat) {
    camera_q
        .single()
        .map(|g| {
            let (_, rot, pos) = g.to_scale_rotation_translation();
            (pos, rot)
        })
        .unwrap_or((Vec3::ZERO, Quat::IDENTITY))
}

#[allow(clippy::too_many_arguments)]
fn dispatch_spatial_sounds(
    mut commands: Commands,
    settings: Res<AudioSettings>,
    colliders: Res<Colliders>,
    audio_sources: Res<Assets<AudioSource>>,
    mut cache: ResMut<MonoAudioCache>,
    mut clips: ResMut<Assets<SpatialClip>>,
    camera_q: Query<&GlobalTransform, With<MainCamera>>,
    global_transforms: Query<&GlobalTransform>,
    mut requests: Query<(Entity, &mut SpatialSoundRequest)>,
    active_q: Query<(Entity, &ActiveSpatialSound)>,
) {
    let (listener_pos, listener_rot) = listener_pose(&camera_q);
    let sfx_scale = settings.effective_sfx();

    let mut live_voices: Vec<(Entity, u8, f32)> = active_q
        .iter()
        .map(|(e, s)| (e, s.priority, s.params.load().loudness_rank))
        .collect();

    for (entity, mut req) in &mut requests {
        let Some(buffer) = cache.get_or_decode(&req.source, &audio_sources) else {
            req.retries = req.retries.saturating_add(1);
            // If an asset never loads after ~120 frames, drop the request.
            if req.retries > 120 {
                commands.entity(entity).despawn();
            }
            continue;
        };

        let sample_rate = buffer.sample_rate.get() as f32;
        let tuning = req.profile.tuning();

        let (emitter, snap) = match req.emitter {
            SpatialEmitter::World(pos) => {
                let occ = is_occluded(&colliders, listener_pos, pos);
                let snap = compute_spatial_params(
                    listener_pos,
                    listener_rot,
                    pos,
                    req.profile,
                    req.base_volume,
                    sfx_scale,
                    occ,
                    sample_rate,
                );
                (SpatialEmitter::World(pos), snap)
            }
            SpatialEmitter::Follow {
                entity: target,
                fallback,
            } => {
                let pos = global_transforms
                    .get(target)
                    .map(|g| g.translation())
                    .unwrap_or(fallback);
                let occ = is_occluded(&colliders, listener_pos, pos);
                let snap = compute_spatial_params(
                    listener_pos,
                    listener_rot,
                    pos,
                    req.profile,
                    req.base_volume,
                    sfx_scale,
                    occ,
                    sample_rate,
                );
                (
                    SpatialEmitter::Follow {
                        entity: target,
                        fallback: pos,
                    },
                    snap,
                )
            }
            SpatialEmitter::HeadRelative { pan } => {
                let snap = compute_head_relative_params(pan, req.base_volume, sfx_scale);
                (SpatialEmitter::HeadRelative { pan }, snap)
            }
        };

        // Out of earshot or muted: skip spawning a silent sink.
        if snap.loudness_rank <= 0.002 {
            commands.entity(entity).despawn();
            continue;
        }

        // Respect voice budget: steal the quietest lower-priority voice if full.
        if live_voices.len() >= MAX_SPATIAL_VOICES {
            let victim = live_voices
                .iter()
                .enumerate()
                .filter(|(_, (_, prio, _))| *prio < tuning.priority)
                .min_by(|(_, (_, p1, l1)), (_, (_, p2, l2))| p1.cmp(p2).then(l1.total_cmp(l2)))
                .map(|(idx, _)| idx);
            if let Some(idx) = victim {
                let (victim_entity, _, _) = live_voices.swap_remove(idx);
                commands.entity(victim_entity).try_despawn();
            } else {
                commands.entity(entity).despawn();
                continue;
            }
        }

        let params = Arc::new(AtomicSpatialParams::new(snap));
        let clip_handle = clips.add(SpatialClip {
            buffer,
            pitch: req.pitch,
            params: params.clone(),
        });

        live_voices.push((entity, tuning.priority, snap.loudness_rank));
        commands
            .entity(entity)
            .remove::<SpatialSoundRequest>()
            .insert((
                AudioPlayer(clip_handle),
                PlaybackSettings::DESPAWN,
                ActiveSpatialSound {
                    params,
                    emitter,
                    profile: req.profile,
                    base_volume: req.base_volume,
                    sample_rate,
                    priority: tuning.priority,
                },
            ));
    }
}

/// Keeps every playing spatial sound locked to the camera's live orientation
/// and position, occlusion state, and `AudioSettings`.
fn update_active_spatial_sounds(
    settings: Res<AudioSettings>,
    colliders: Res<Colliders>,
    camera_q: Query<&GlobalTransform, With<MainCamera>>,
    global_transforms: Query<&GlobalTransform>,
    mut active: Query<&mut ActiveSpatialSound>,
) {
    let (listener_pos, listener_rot) = listener_pose(&camera_q);
    let sfx_scale = settings.effective_sfx();

    for mut sound in &mut active {
        let snap = match &mut sound.emitter {
            SpatialEmitter::World(pos) => {
                let occ = is_occluded(&colliders, listener_pos, *pos);
                compute_spatial_params(
                    listener_pos,
                    listener_rot,
                    *pos,
                    sound.profile,
                    sound.base_volume,
                    sfx_scale,
                    occ,
                    sound.sample_rate,
                )
            }
            SpatialEmitter::Follow { entity, fallback } => {
                if let Ok(g) = global_transforms.get(*entity) {
                    *fallback = g.translation();
                }
                let pos = *fallback;
                let occ = is_occluded(&colliders, listener_pos, pos);
                compute_spatial_params(
                    listener_pos,
                    listener_rot,
                    pos,
                    sound.profile,
                    sound.base_volume,
                    sfx_scale,
                    occ,
                    sound.sample_rate,
                )
            }
            SpatialEmitter::HeadRelative { pan } => {
                compute_head_relative_params(*pan, sound.base_volume, sfx_scale)
            }
        };
        sound.params.store(snap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_on_left_plays_louder_and_earlier_in_left_ear() {
        // Default camera at origin looks down -Z, so -X is LEFT and +X is RIGHT.
        let snap = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(-10.0, 0.0, 0.0),
            SoundProfile::Gunshot,
            1.0,
            1.0,
            false,
            48_000.0,
        );
        assert!(
            snap.left_gain > snap.right_gain * 4.0,
            "left ear should dominate for a source on the left: L={}, R={}",
            snap.left_gain,
            snap.right_gain
        );
        assert!(
            snap.itd_samples < -10.0,
            "left ear should lead (negative ITD): {}",
            snap.itd_samples
        );
        assert!(
            snap.left_lpf > snap.right_lpf,
            "right ear should be head-shadowed: L_lpf={}, R_lpf={}",
            snap.left_lpf,
            snap.right_lpf
        );
    }

    #[test]
    fn sound_on_right_plays_louder_and_earlier_in_right_ear() {
        let snap = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(10.0, 0.0, 0.0),
            SoundProfile::Footstep,
            1.0,
            1.0,
            false,
            44_100.0,
        );
        assert!(
            snap.right_gain > snap.left_gain * 4.0,
            "right ear should dominate for a source on the right: L={}, R={}",
            snap.left_gain,
            snap.right_gain
        );
        assert!(
            snap.itd_samples > 10.0,
            "right ear should lead (positive ITD): {}",
            snap.itd_samples
        );
        assert!(snap.right_lpf > snap.left_lpf);
    }

    #[test]
    fn turning_camera_180_degrees_flips_earbuds() {
        let world_source = Vec3::new(-8.0, 0.0, 0.0);
        // Facing -Z (yaw = 0): (-8, 0, 0) is on the player's LEFT.
        let front = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            world_source,
            SoundProfile::Gunshot,
            1.0,
            1.0,
            false,
            48_000.0,
        );
        // Facing +Z (yaw = PI): (-8, 0, 0) is now on the player's RIGHT!
        let turned = compute_spatial_params(
            Vec3::ZERO,
            Quat::from_rotation_y(std::f32::consts::PI),
            world_source,
            SoundProfile::Gunshot,
            1.0,
            1.0,
            false,
            48_000.0,
        );
        assert!(front.left_gain > front.right_gain * 4.0);
        assert!(turned.right_gain > turned.left_gain * 4.0);
        assert!((front.left_gain - turned.right_gain).abs() < 1e-4);
    }

    #[test]
    fn sound_ahead_vs_behind_is_centered_and_behind_is_darker() {
        let ahead = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(0.0, 0.0, -8.0),
            SoundProfile::Footstep,
            1.0,
            1.0,
            false,
            44_100.0,
        );
        let behind = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(0.0, 0.0, 8.0),
            SoundProfile::Footstep,
            1.0,
            1.0,
            false,
            44_100.0,
        );
        assert!((ahead.left_gain - ahead.right_gain).abs() < 1e-5);
        assert!((behind.left_gain - behind.right_gain).abs() < 1e-5);
        assert!(ahead.itd_samples.abs() < 1e-5);
        assert!(behind.itd_samples.abs() < 1e-5);
        assert!(behind.left_lpf < ahead.left_lpf);
        assert!(behind.left_gain < ahead.left_gain);
    }

    #[test]
    fn occlusion_attenuates_and_muffles_sound() {
        let clear = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(6.0, 0.0, -4.0),
            SoundProfile::Footstep,
            1.0,
            1.0,
            false,
            44_100.0,
        );
        let blocked = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(6.0, 0.0, -4.0),
            SoundProfile::Footstep,
            1.0,
            1.0,
            true,
            44_100.0,
        );
        assert!(blocked.right_gain < clear.right_gain * 0.7);
        assert!(blocked.right_lpf < clear.right_lpf * 0.5);
    }

    #[test]
    fn spatial_decoder_interleaves_left_channel_0_and_right_channel_1_correctly() {
        // Constant DC test tone on the LEFT: channel 0 (even) must exceed channel 1 (odd).
        let buf = Arc::new(MonoBuffer {
            samples: vec![0.5; 2000],
            sample_rate: SampleRate::new(48_000).unwrap(),
        });
        let snap = compute_spatial_params(
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(-8.0, 0.0, 0.0),
            SoundProfile::Gunshot,
            1.0,
            1.0,
            false,
            48_000.0,
        );
        let decoder = SpatialDecoder::new(buf, 1.0, Arc::new(AtomicSpatialParams::new(snap)));
        let out: Vec<f32> = decoder.collect();
        assert_eq!(out.len() % 2, 0, "stereo output must end on a complete frame");
        let mid_l = out[1000];
        let mid_r = out[1001];
        assert!(
            mid_l > mid_r * 4.0,
            "channel 0 (left) must be louder than channel 1 (right): L={mid_l}, R={mid_r}"
        );
    }
}

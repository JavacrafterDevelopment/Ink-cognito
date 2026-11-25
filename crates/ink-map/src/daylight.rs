//! Time of day: how a map is lit.
//!
//! A map file stores only its sky color ([`crate::MapFile::sky`]). Each sky
//! preset in [`TIMES`] stands for a time of day, and [`Daylight::for_sky`]
//! turns the sky into the sun, ambient light and fog to light the map with,
//! by the preset nearest that color. The game and the map editor both light
//! from this, so what the editor shows is what players get. All lights are
//! real time (nothing is baked), so moving pieces never needs a re-bake.

use bevy_math::Vec3;

/// The times of day a map can be set to: name and sky color (sRGB 0..1).
/// The first six are the sky presets older maps were made with.
pub const TIMES: [(&str, [f32; 3]); 7] = [
    ("Day", [0.55, 0.76, 0.96]),
    ("Clear", [0.6, 0.78, 0.95]),
    ("Pale", [0.66, 0.8, 0.95]),
    ("Haze", [0.7, 0.82, 0.9]),
    ("Sunset", [0.93, 0.72, 0.56]),
    ("Dusk", [0.36, 0.38, 0.55]),
    ("Night", [0.035, 0.05, 0.11]),
];

/// The preset in `presets` closest to color `c`.
pub fn nearest(presets: &[(&str, [f32; 3])], c: [f32; 3]) -> usize {
    let d = |p: [f32; 3]| (0..3).map(|i| (p[i] - c[i]).abs()).sum::<f32>();
    presets
        .iter()
        .enumerate()
        .min_by(|a, b| d(a.1.1).total_cmp(&d(b.1.1)))
        .map_or(0, |(i, _)| i)
}

/// Distance fog: color (sRGB) and where it starts and is fully thick (meters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

/// How to light a map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daylight {
    /// The background color (the sky), sRGB.
    pub sky: [f32; 3],
    /// Sun (or moon) color, sRGB.
    pub sun_color: [f32; 3],
    /// Sun (or moon) strength, lux.
    pub sun_lux: f32,
    /// Degrees above the horizon.
    pub sun_elevation: f32,
    /// Degrees around the field, from +x toward +z.
    pub sun_azimuth: f32,
    /// Light from everywhere, sRGB.
    pub ambient_color: [f32; 3],
    /// Bevy's `GlobalAmbientLight::brightness`.
    pub ambient_brightness: f32,
    /// None: clear air.
    pub fog: Option<Fog>,
}

impl Daylight {
    /// The light for a map with this sky color.
    pub fn for_sky(sky: [f32; 3]) -> Self {
        let (
            sun_color,
            sun_lux,
            sun_elevation,
            sun_azimuth,
            ambient_color,
            ambient_brightness,
            fog,
        ) = match TIMES[nearest(&TIMES, sky)].0 {
            "Clear" => (
                [1.0, 0.98, 0.93],
                15_500.0,
                58.0,
                40.0,
                [0.88, 0.93, 1.0],
                380.0,
                None,
            ),
            "Pale" => (
                [1.0, 0.98, 0.95],
                12_000.0,
                52.0,
                40.0,
                [0.9, 0.94, 1.0],
                420.0,
                None,
            ),
            "Haze" => (
                [1.0, 0.96, 0.9],
                9_000.0,
                45.0,
                40.0,
                [0.9, 0.92, 0.94],
                460.0,
                Some(Fog {
                    color: [0.7, 0.82, 0.9],
                    start: 45.0,
                    end: 220.0,
                }),
            ),
            "Sunset" => (
                [1.0, 0.62, 0.36],
                6_500.0,
                11.0,
                20.0,
                [1.0, 0.8, 0.68],
                230.0,
                Some(Fog {
                    color: [0.93, 0.72, 0.56],
                    start: 60.0,
                    end: 260.0,
                }),
            ),
            "Dusk" => (
                [0.72, 0.62, 0.9],
                1_800.0,
                6.0,
                200.0,
                [0.55, 0.58, 0.8],
                130.0,
                Some(Fog {
                    color: [0.36, 0.38, 0.55],
                    start: 40.0,
                    end: 200.0,
                }),
            ),
            // Moonlight: dim and blue, from high up so the field still reads.
            "Night" => (
                [0.62, 0.72, 1.0],
                650.0,
                55.0,
                235.0,
                [0.32, 0.4, 0.7],
                55.0,
                Some(Fog {
                    color: [0.035, 0.05, 0.11],
                    start: 30.0,
                    end: 150.0,
                }),
            ),
            // "Day", and the light the game always had.
            _ => (
                [1.0, 0.97, 0.9],
                14_000.0,
                50.0,
                40.0,
                [0.88, 0.93, 1.0],
                380.0,
                None,
            ),
        };
        Self {
            sky,
            sun_color,
            sun_lux,
            sun_elevation,
            sun_azimuth,
            ambient_color,
            ambient_brightness,
            fog,
        }
    }

    /// Where to put the sun light so it shines down on the field center
    /// (point it at the origin).
    pub fn sun_position(&self) -> Vec3 {
        let (e, a) = (
            self.sun_elevation.to_radians(),
            self.sun_azimuth.to_radians(),
        );
        Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin()) * 50.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_is_darker_than_day() {
        let day = Daylight::for_sky(TIMES[0].1);
        let night = Daylight::for_sky(TIMES[6].1);
        assert!(night.sun_lux < day.sun_lux / 10.0);
        assert!(night.ambient_brightness < day.ambient_brightness / 4.0);
        assert!(night.fog.is_some() && day.fog.is_none());
    }

    #[test]
    fn day_keeps_the_old_light() {
        let d = Daylight::for_sky([0.55, 0.76, 0.96]);
        assert_eq!(d.sun_lux, 14_000.0);
        assert_eq!(d.ambient_brightness, 380.0);
    }

    #[test]
    fn sun_is_above_the_ground() {
        for (_, sky) in TIMES {
            assert!(Daylight::for_sky(sky).sun_position().y > 0.0);
        }
    }
}

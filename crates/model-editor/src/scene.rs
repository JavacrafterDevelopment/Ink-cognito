//! Syncs the `ModelDoc` (parts, materials, editable RGBA textures) into live
//! Bevy 3D meshes, PBR materials, and GPU images, plus 3-point studio lighting.

use crate::{
    model::{AlphaKind, MeshPart, TextureSlot},
    state::{Editor, LightPreset},
};
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{Extent3d, Face, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RenderCache>()
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.90, 0.94, 1.0),
                brightness: 380.0,
                ..default()
            })
            .add_systems(Startup, setup_lights)
            .add_systems(Update, (sync_scene, sync_lighting).chain());
    }
}

/// Tag component for the 3D entity rendering `doc.parts[index]`.
#[derive(Component)]
pub struct PartEntity {
    pub part_id: u64,
    pub mesh_rev: u64,
    pub mesh_handle: Handle<Mesh>,
}

#[derive(Component)]
struct KeyLight;

#[derive(Component)]
struct FillLight;

#[derive(Component)]
struct RimLight;

/// GPU asset handles kept in sync with `ModelDoc`.
#[derive(Resource, Default)]
pub struct RenderCache {
    /// sRGB image handles indexed by `doc.textures` index (used for BaseColor,
    /// Emissive, and the 2D Texture Preview in the Paint UI).
    pub srgb_textures: Vec<(u64, u32, u32, Handle<Image>)>,
    /// Linear image handles for Normal and Metallic/Roughness texture slots.
    pub linear_textures: HashMap<usize, (u64, u32, u32, Handle<Image>)>,
    /// Material handles indexed by `doc.materials` index.
    pub materials: Vec<Handle<StandardMaterial>>,
}

impl RenderCache {
    pub fn preview_handle(&self, tex_idx: usize) -> Option<Handle<Image>> {
        self.srgb_textures.get(tex_idx).map(|e| e.3.clone())
    }
}

fn setup_lights(mut commands: Commands) {
    // Key light
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.97, 0.92),
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(6.0, 10.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y),
        KeyLight,
    ));
    // Soft cool fill light
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.76, 0.86, 1.0),
            illuminance: 3_200.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-8.0, 5.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
        FillLight,
    ));
    // Subtle rim light
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.95, 0.90, 1.0),
            illuminance: 2_200.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(0.0, 4.0, -9.0).looking_at(Vec3::ZERO, Vec3::Y),
        RimLight,
    ));
}

fn make_image(tex: &TextureSlot, format: TextureFormat) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: tex.width.max(1),
            height: tex.height.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        if tex.width == 0 || tex.height == 0 {
            vec![255, 255, 255, 255]
        } else {
            (*tex.pixels).clone()
        },
        format,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

fn update_image_in_place(images: &mut Assets<Image>, handle: &Handle<Image>, tex: &TextureSlot) -> bool {
    if let Some(mut img) = images.get_mut(handle) {
        if let Some(data) = &mut img.data
            && data.len() == tex.pixels.len()
        {
            data.copy_from_slice(&tex.pixels);
            return true;
        }
    }
    false
}

fn build_bevy_mesh(part: &MeshPart) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions.clone());

    let normals = if part.normals.len() == part.positions.len() {
        part.normals.clone()
    } else {
        vec![[0.0, 1.0, 0.0]; part.positions.len()]
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);

    if part.has_uvs() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs.clone());
    } else {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; part.positions.len()]);
    }
    if let Some(colors) = &part.colors
        && colors.len() == part.positions.len()
    {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors.clone());
    }
    if let Some(tangents) = &part.tangents
        && tangents.len() == part.positions.len()
    {
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents.clone());
    }
    mesh.insert_indices(Indices::U32(part.indices.clone()));
    mesh
}

/// Incrementally synchronizes textures, materials, meshes, transforms, and
/// visibility from `Editor.doc` to Bevy entities.
#[allow(clippy::too_many_arguments)]
fn sync_scene(
    mut commands: Commands,
    editor: Res<Editor>,
    mut cache: ResMut<RenderCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut existing: Query<(Entity, &mut PartEntity, &mut Transform, &mut MeshMaterial3d<StandardMaterial>, &mut Visibility)>,
) {
    let doc = &editor.doc;

    // 1. Sync sRGB textures (fast in-place buffer copy on paint strokes!).
    cache.srgb_textures.truncate(doc.textures.len());
    for (i, tex) in doc.textures.iter().enumerate() {
        if i < cache.srgb_textures.len() {
            let entry = &mut cache.srgb_textures[i];
            if entry.0 != tex.revision || entry.1 != tex.width || entry.2 != tex.height {
                if entry.1 == tex.width && entry.2 == tex.height && update_image_in_place(&mut images, &entry.3, tex) {
                    entry.0 = tex.revision;
                } else {
                    let h = images.add(make_image(tex, TextureFormat::Rgba8UnormSrgb));
                    *entry = (tex.revision, tex.width, tex.height, h);
                }
            }
        } else {
            let h = images.add(make_image(tex, TextureFormat::Rgba8UnormSrgb));
            cache.srgb_textures.push((tex.revision, tex.width, tex.height, h));
        }
    }

    // 2. Sync linear textures only for slots referenced as Normal / MetallicRoughness.
    let mut need_linear = Vec::new();
    for m in &doc.materials {
        if let Some(ti) = m.normal_texture
            && ti < doc.textures.len()
        {
            need_linear.push(ti);
        }
        if let Some(ti) = m.metallic_roughness_texture
            && ti < doc.textures.len()
        {
            need_linear.push(ti);
        }
    }
    cache.linear_textures.retain(|&ti, _| ti < doc.textures.len() && need_linear.contains(&ti));
    for ti in need_linear {
        let tex = &doc.textures[ti];
        match cache.linear_textures.get_mut(&ti) {
            Some(entry) => {
                if entry.0 != tex.revision || entry.1 != tex.width || entry.2 != tex.height {
                    if entry.1 == tex.width && entry.2 == tex.height && update_image_in_place(&mut images, &entry.3, tex) {
                        entry.0 = tex.revision;
                    } else {
                        let h = images.add(make_image(tex, TextureFormat::Rgba8Unorm));
                        *entry = (tex.revision, tex.width, tex.height, h);
                    }
                }
            }
            None => {
                let h = images.add(make_image(tex, TextureFormat::Rgba8Unorm));
                cache.linear_textures.insert(ti, (tex.revision, tex.width, tex.height, h));
            }
        }
    }

    // 3. Sync PBR materials.
    cache.materials.truncate(doc.materials.len());
    while cache.materials.len() < doc.materials.len() {
        cache.materials.push(materials.add(StandardMaterial::default()));
    }
    for (i, slot) in doc.materials.iter().enumerate() {
        if let Some(mut mat) = materials.get_mut(&cache.materials[i]) {
            mat.base_color = Color::linear_rgba(
                slot.base_color[0],
                slot.base_color[1],
                slot.base_color[2],
                slot.base_color[3],
            );
            mat.metallic = slot.metallic.clamp(0.0, 1.0);
            mat.perceptual_roughness = slot.roughness.clamp(0.04, 1.0);
            mat.emissive = LinearRgba::rgb(slot.emissive[0], slot.emissive[1], slot.emissive[2]);
            mat.double_sided = slot.double_sided;
            mat.cull_mode = if slot.double_sided { None } else { Some(Face::Back) };
            mat.alpha_mode = match slot.alpha_mode {
                AlphaKind::Opaque => AlphaMode::Opaque,
                AlphaKind::Mask(c) => AlphaMode::Mask(c),
                AlphaKind::Blend => AlphaMode::Blend,
            };
            mat.base_color_texture = slot
                .base_color_texture
                .and_then(|ti| cache.srgb_textures.get(ti))
                .map(|e| e.3.clone());
            mat.emissive_texture = slot
                .emissive_texture
                .and_then(|ti| cache.srgb_textures.get(ti))
                .map(|e| e.3.clone());
            mat.normal_map_texture = slot
                .normal_texture
                .and_then(|ti| cache.linear_textures.get(&ti))
                .map(|e| e.3.clone());
            mat.metallic_roughness_texture = slot
                .metallic_roughness_texture
                .and_then(|ti| cache.linear_textures.get(&ti))
                .map(|e| e.3.clone());
        }
    }

    // 4. Sync Part entities by stable `part.id`.
    let mut by_id: HashMap<u64, Entity> = HashMap::new();
    let live_ids: HashMap<u64, usize> = doc.parts.iter().enumerate().map(|(i, p)| (p.id, i)).collect();

    for (entity, mut pe, mut tf, mut mat_comp, mut vis) in &mut existing {
        let Some(&idx) = live_ids.get(&pe.part_id) else {
            commands.entity(entity).despawn();
            continue;
        };
        by_id.insert(pe.part_id, entity);
        let part = &doc.parts[idx];
        let want_tf = Transform {
            translation: Vec3::from_array(part.translation),
            rotation: part.quat(),
            scale: Vec3::from_array(part.scale),
        };
        if *tf != want_tf {
            *tf = want_tf;
        }
        let want_vis = if part.visible { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want_vis {
            *vis = want_vis;
        }
        if let Some(mh) = cache.materials.get(part.material)
            && mat_comp.0 != *mh
        {
            mat_comp.0 = mh.clone();
        }
        if pe.mesh_rev != part.revision {
            if let Some(mut gpu_mesh) = meshes.get_mut(&pe.mesh_handle) {
                *gpu_mesh = build_bevy_mesh(part);
            }
            pe.mesh_rev = part.revision;
        }
    }

    // Spawn newly added parts.
    for part in &doc.parts {
        if by_id.contains_key(&part.id) {
            continue;
        }
        let mesh_handle = meshes.add(build_bevy_mesh(part));
        let mat_handle = cache
            .materials
            .get(part.material)
            .cloned()
            .unwrap_or_else(|| materials.add(StandardMaterial::default()));
        commands.spawn((
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(mat_handle),
            Transform {
                translation: Vec3::from_array(part.translation),
                rotation: part.quat(),
                scale: Vec3::from_array(part.scale),
            },
            if part.visible { Visibility::Inherited } else { Visibility::Hidden },
            PartEntity {
                part_id: part.id,
                mesh_rev: part.revision,
                mesh_handle,
            },
        ));
    }
}

fn sync_lighting(
    editor: Res<Editor>,
    mut clear: ResMut<ClearColor>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut key_q: Query<&mut DirectionalLight, (With<KeyLight>, Without<FillLight>, Without<RimLight>)>,
    mut fill_q: Query<&mut DirectionalLight, (With<FillLight>, Without<KeyLight>, Without<RimLight>)>,
    mut rim_q: Query<&mut DirectionalLight, (With<RimLight>, Without<KeyLight>, Without<FillLight>)>,
    mut last: Local<Option<LightPreset>>,
) {
    if *last == Some(editor.light_preset) {
        return;
    }
    *last = Some(editor.light_preset);
    let (bg, amb_c, amb_b, key_i, fill_i, rim_i) = match editor.light_preset {
        LightPreset::Studio => (
            Color::srgb(0.11, 0.14, 0.19),
            Color::srgb(0.90, 0.94, 1.0),
            420.0,
            10_000.0,
            3_500.0,
            2_500.0,
        ),
        LightPreset::Daylight => (
            Color::srgb(0.46, 0.66, 0.88),
            Color::srgb(0.92, 0.96, 1.0),
            650.0,
            16_000.0,
            4_500.0,
            1_800.0,
        ),
        LightPreset::Bright => (
            Color::srgb(0.24, 0.27, 0.32),
            Color::srgb(1.0, 1.0, 1.0),
            1_100.0,
            7_500.0,
            5_500.0,
            4_000.0,
        ),
        LightPreset::Dark => (
            Color::srgb(0.03, 0.04, 0.06),
            Color::srgb(0.65, 0.72, 0.85),
            120.0,
            6_500.0,
            1_200.0,
            3_200.0,
        ),
    };
    clear.0 = bg;
    ambient.color = amb_c;
    ambient.brightness = amb_b;
    if let Ok(mut l) = key_q.single_mut() {
        l.illuminance = key_i;
    }
    if let Ok(mut l) = fill_q.single_mut() {
        l.illuminance = fill_i;
    }
    if let Ok(mut l) = rim_q.single_mut() {
        l.illuminance = rim_i;
    }
}

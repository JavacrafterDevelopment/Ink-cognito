//! The game's UI kit: 9-slice buttons, panels and rows from
//! `assets/textures/ui-kit`, and the Anton / Archivo Black fonts.
//!
//! Every button made with [`crate::menu::spawn_button`] (or anything carrying
//! a [`ButtonStyle`]) is drawn with the kit's button art and swaps to its
//! hover / pressed art; every card that uses [`BG_CARD`] becomes a kit panel;
//! every text without a font of its own gets Anton (big) or Archivo Black.

use crate::menu::{BG_CARD, BTN_DANGER, BTN_PRIMARY, ButtonStyle};
use bevy::{picking::prelude::*, prelude::*, text::FontSource};

pub const KIT: &str = "textures/ui-kit";

pub const KIT_YELLOW: Color = Color::srgb(0.96, 0.77, 0.17);
pub const KIT_DARK_TEXT: Color = Color::srgb(0.071, 0.082, 0.094);

/// Text this size or bigger uses the heading font.
const HEADING_SIZE: f32 = 26.0;

pub struct UiKitPlugin;

impl Plugin for UiKitPlugin {
    fn build(&self, app: &mut App) {
        let assets = app.world().resource::<AssetServer>().clone();
        app.insert_resource(UiKit {
            heading: assets.load("fonts/Anton-Regular.ttf"),
            body: assets.load("fonts/ArchivoBlack-Regular.ttf"),
        })
        .add_observer(on_kit_over)
        .add_observer(on_kit_out)
        .add_observer(on_kit_press)
        .add_observer(on_kit_release)
        .add_systems(PostUpdate, (style_buttons, style_cards, apply_fonts));
    }
}

#[derive(Resource, Clone)]
pub struct UiKit {
    pub heading: Handle<Font>,
    pub body: Handle<Font>,
}

impl UiKit {
    pub fn heading(&self, size: f32) -> TextFont {
        TextFont {
            font: self.heading.clone().into(),
            font_size: FontSize::Px(size),
            ..default()
        }
    }
}

/// `textures/ui-kit/<path>`.
pub fn kit(assets: &AssetServer, path: &str) -> Handle<Image> {
    assets.load(format!("{KIT}/{path}"))
}

/// An image stretched as a 9-slice with the same border all round.
pub fn sliced(image: Handle<Image>, border: f32) -> ImageNode {
    sliced_rect(image, BorderRect::all(border))
}

pub fn sliced_rect(image: Handle<Image>, border: BorderRect) -> ImageNode {
    ImageNode {
        // Behind the padding too: the art is the whole box.
        visual_box: VisualBox::BorderBox,
        ..ImageNode::new(image).with_mode(NodeImageMode::Sliced(TextureSlicer {
            border,
            center_scale_mode: SliceScaleMode::Stretch,
            sides_scale_mode: SliceScaleMode::Stretch,
            max_corner_scale: 1.0,
        }))
    }
}

/// The kit's button colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Secondary,
    Danger,
}

impl Btn {
    fn file(self) -> &'static str {
        match self {
            Btn::Primary => "primary",
            Btn::Secondary => "secondary",
            Btn::Danger => "danger",
        }
    }

    pub fn text_color(self) -> Color {
        match self {
            Btn::Primary => KIT_DARK_TEXT,
            _ => Color::srgb(0.96, 0.97, 1.0),
        }
    }

    /// Which kit button an old colour style maps to.
    pub fn of(style: &ButtonStyle) -> Btn {
        if style.normal == BTN_PRIMARY {
            Btn::Primary
        } else if style.normal == BTN_DANGER {
            Btn::Danger
        } else {
            Btn::Secondary
        }
    }
}

/// Button art for each state.
#[derive(Component, Clone)]
pub struct KitButton {
    pub kind: Btn,
    pub normal: Handle<Image>,
    pub hover: Handle<Image>,
    pub pressed: Handle<Image>,
}

impl KitButton {
    pub fn new(assets: &AssetServer, kind: Btn) -> Self {
        let f = kind.file();
        KitButton {
            kind,
            normal: kit(assets, &format!("buttons/button_{f}_normal.png")),
            hover: kit(assets, &format!("buttons/button_{f}_hover.png")),
            pressed: kit(assets, &format!("buttons/button_{f}_pressed.png")),
        }
    }

    pub fn image(&self) -> ImageNode {
        sliced(self.normal.clone(), 20.0)
    }
}

fn set_image(
    q: &mut Query<(&KitButton, &mut ImageNode)>,
    e: Entity,
    pick: fn(&KitButton) -> Handle<Image>,
) {
    if let Ok((b, mut img)) = q.get_mut(e) {
        let h = pick(b);
        if img.image != h {
            img.image = h;
        }
    }
}

fn on_kit_over(over: On<PointerOver>, mut q: Query<(&KitButton, &mut ImageNode)>) {
    set_image(&mut q, over.entity, |b| b.hover.clone());
}

fn on_kit_out(out: On<PointerOut>, mut q: Query<(&KitButton, &mut ImageNode)>) {
    set_image(&mut q, out.entity, |b| b.normal.clone());
}

fn on_kit_press(press: On<PointerPress>, mut q: Query<(&KitButton, &mut ImageNode)>) {
    if press.button == PointerButton::Primary {
        set_image(&mut q, press.entity, |b| b.pressed.clone());
    }
}

fn on_kit_release(release: On<PointerRelease>, mut q: Query<(&KitButton, &mut ImageNode)>) {
    if release.button == PointerButton::Primary {
        set_image(&mut q, release.entity, |b| b.hover.clone());
    }
}

/// Gives every [`ButtonStyle`] button the kit's art (and keeps it in step
/// when its style changes, like a selected tab).
#[allow(clippy::type_complexity)]
fn style_buttons(
    mut commands: Commands,
    assets: Res<AssetServer>,
    buttons: Query<
        (Entity, &ButtonStyle, Option<&KitButton>, Option<&Children>),
        Changed<ButtonStyle>,
    >,
    mut texts: Query<&mut TextColor>,
    mut bgs: Query<&mut BackgroundColor>,
) {
    for (e, style, existing, children) in &buttons {
        let kind = Btn::of(style);
        if existing.is_none_or(|k| k.kind != kind) {
            let kb = KitButton::new(&assets, kind);
            commands.entity(e).insert((kb.image(), kb));
        }
        if let Ok(mut bg) = bgs.get_mut(e) {
            bg.0 = Color::NONE;
        }
        for &child in children.into_iter().flatten() {
            if let Ok(mut c) = texts.get_mut(child) {
                c.0 = kind.text_color();
            }
        }
    }
}

/// Old dark cards become kit panels.
#[allow(clippy::type_complexity)]
fn style_cards(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut cards: Query<
        (Entity, &mut BackgroundColor),
        (
            Added<BackgroundColor>,
            Without<ButtonStyle>,
            Without<ImageNode>,
        ),
    >,
) {
    for (e, mut bg) in &mut cards {
        if bg.0 != BG_CARD {
            continue;
        }
        bg.0 = Color::NONE;
        commands.entity(e).insert((
            sliced(kit(&assets, "panels/panel.png"), 28.0),
            BorderColor::all(Color::NONE),
        ));
    }
}

/// Text without a font of its own gets the kit's fonts.
fn apply_fonts(kit: Res<UiKit>, mut texts: Query<&mut TextFont, Added<TextFont>>) {
    for mut font in &mut texts {
        let FontSource::Handle(h) = &font.font else {
            continue;
        };
        if *h != Handle::<Font>::default() {
            continue;
        }
        let size = match font.font_size {
            FontSize::Px(s) => s,
            _ => 16.0,
        };
        font.font = if size >= HEADING_SIZE {
            kit.heading.clone()
        } else {
            kit.body.clone()
        }
        .into();
    }
}

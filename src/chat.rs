//! In-game text chat: press Enter (rebindable under Keybinds) to type a line,
//! Enter again to send it to everyone, Escape to cancel. While typing you
//! don't move or shoot (`Overlay::chat`), but the mouse stays captured.
//!
//! Lines go through the host (`NetworkState::send_chat`), which passes each on
//! to every player, so everyone sees them in the same order. Recent lines show
//! above the health panel and fade after a while; all of them come back while
//! the chat box is open.

use crate::{
    AppState, Overlay,
    net::NetworkState,
    protocol::{MAX_CHAT_CHARS, PlayerId},
    settings::{KeyAction, KeybindSettings},
};
use bevy::{
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    prelude::*,
};
use std::collections::VecDeque;

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatState>()
            .add_systems(OnEnter(AppState::InGame), spawn_chat)
            .add_systems(OnExit(AppState::InGame), reset_chat)
            .add_systems(
                Update,
                (receive_chat, chat_keys, update_chat_ui)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// Lines shown at once.
const ROWS: usize = 8;
/// Lines remembered.
const KEEP: usize = 50;
/// A line stays up this long (seconds) while the chat box is closed...
const SHOW_FOR: f32 = 10.0;
/// ...then fades out over this long.
const FADE_FOR: f32 = 1.5;

struct ChatLine {
    from: PlayerId,
    text: String,
    /// When it arrived (`Time::elapsed_secs`).
    at: f32,
}

#[derive(Resource, Default)]
pub struct ChatState {
    lines: VecDeque<ChatLine>,
    /// What's being typed.
    draft: String,
}

#[derive(Component)]
struct ChatRow(usize);

#[derive(Component)]
struct ChatRowName(usize);

#[derive(Component)]
struct ChatRowText(usize);

#[derive(Component)]
struct ChatInputBox;

#[derive(Component)]
struct ChatInputText;

/// What a player is called in chat.
pub fn chat_name(id: PlayerId) -> String {
    crate::hud::player_name(id, None)
}

fn reset_chat(mut chat: ResMut<ChatState>) {
    *chat = ChatState::default();
}

fn spawn_chat(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(20.0),
                bottom: Val::Px(170.0),
                width: Val::Px(440.0),
                max_width: Val::Percent(60.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            Pickable::IGNORE,
            GlobalZIndex(4),
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            for i in 0..ROWS {
                root.spawn((
                    Node {
                        padding: UiRect::axes(Val::Px(8.0), Val::Px(3.0)),
                        column_gap: Val::Px(6.0),
                        flex_wrap: FlexWrap::Wrap,
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        display: Display::None,
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                    Pickable::IGNORE,
                    ChatRow(i),
                ))
                .with_children(|row| {
                    row.spawn((
                        Text::new(""),
                        TextFont::from_font_size(15.0),
                        TextColor(Color::WHITE),
                        TextShadow::default(),
                        Pickable::IGNORE,
                        ChatRowName(i),
                    ));
                    row.spawn((
                        Text::new(""),
                        TextFont::from_font_size(15.0),
                        TextColor(Color::WHITE),
                        TextShadow::default(),
                        Pickable::IGNORE,
                        ChatRowText(i),
                    ));
                });
            }

            root.spawn((
                Node {
                    margin: UiRect::top(Val::Px(4.0)),
                    padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(6.0)),
                    display: Display::None,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.03, 0.05, 0.08, 0.78)),
                BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.35)),
                Pickable::IGNORE,
                ChatInputBox,
            ))
            .with_children(|input| {
                input.spawn((
                    Text::new(""),
                    TextFont::from_font_size(15.0),
                    TextColor(Color::WHITE),
                    Pickable::IGNORE,
                    ChatInputText,
                ));
            });
        });
}

/// Lines the network heard go into the chat.
fn receive_chat(time: Res<Time>, mut net: ResMut<NetworkState>, mut chat: ResMut<ChatState>) {
    if net.pending_chat.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    for (from, text) in net.pending_chat.drain(..) {
        chat.lines.push_back(ChatLine {
            from,
            text,
            at: now,
        });
    }
    while chat.lines.len() > KEEP {
        chat.lines.pop_front();
    }
}

/// Opens the chat box on the chat key; while it's open, typing goes into the
/// line, Enter sends it and Escape cancels.
fn chat_keys(
    mut key_events: MessageReader<KeyboardInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keybinds: Res<KeybindSettings>,
    net: Res<NetworkState>,
    mut overlay: ResMut<Overlay>,
    mut chat: ResMut<ChatState>,
) {
    if !overlay.chat {
        let open = !overlay.menu_open() && keybinds.just_pressed(KeyAction::Chat, &keys, &mouse);
        // The key that opened it isn't typed into the line.
        key_events.clear();
        if open {
            overlay.chat = true;
            chat.draft.clear();
        }
        return;
    }
    if overlay.menu_open() {
        overlay.chat = false;
        key_events.clear();
        return;
    }

    for event in key_events.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        match &event.logical_key {
            Key::Enter => {
                net.send_chat(&chat.draft);
                chat.draft.clear();
                overlay.chat = false;
                // Nothing else acts on this press.
                keys.clear_just_pressed(event.key_code);
            }
            Key::Escape => {
                chat.draft.clear();
                overlay.chat = false;
                // Escape closes the chat, not opens the Escape menu too.
                keys.clear_just_pressed(event.key_code);
            }
            Key::Backspace => {
                chat.draft.pop();
            }
            _ => {
                if let Some(text) = &event.text {
                    for ch in text.chars() {
                        if !ch.is_control() && chat.draft.chars().count() < MAX_CHAT_CHARS {
                            chat.draft.push(ch);
                        }
                    }
                }
            }
        }
        if !overlay.chat {
            key_events.clear();
            break;
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_chat_ui(
    time: Res<Time>,
    net: Res<NetworkState>,
    overlay: Res<Overlay>,
    chat: Res<ChatState>,
    mut rows: Query<(&ChatRow, &mut Node, &mut BackgroundColor), Without<ChatInputBox>>,
    mut names: Query<(&ChatRowName, &mut Text, &mut TextColor), Without<ChatRowText>>,
    mut texts: Query<
        (&ChatRowText, &mut Text, &mut TextColor),
        (Without<ChatRowName>, Without<ChatInputText>),
    >,
    mut input_box: Query<&mut Node, (With<ChatInputBox>, Without<ChatRow>)>,
    mut input_text: Query<
        &mut Text,
        (
            With<ChatInputText>,
            Without<ChatRowName>,
            Without<ChatRowText>,
        ),
    >,
) {
    let now = time.elapsed_secs();
    let open = overlay.chat;
    let shown = chat.lines.len().min(ROWS);
    let first = chat.lines.len() - shown;

    // How visible row `i` is (0 when it has no line or has faded).
    let alpha_of = |i: usize| -> f32 {
        if i < ROWS - shown {
            return 0.0;
        }
        let line = &chat.lines[first + i - (ROWS - shown)];
        if open {
            1.0
        } else {
            let age = now - line.at;
            (1.0 - (age - SHOW_FOR) / FADE_FOR).clamp(0.0, 1.0)
        }
    };
    let line_of = |i: usize| -> Option<&ChatLine> {
        (i >= ROWS - shown).then(|| &chat.lines[first + i - (ROWS - shown)])
    };

    for (row, mut node, mut bg) in &mut rows {
        let a = alpha_of(row.0);
        let display = if a > 0.0 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
        bg.0 = Color::srgba(0.03, 0.05, 0.08, 0.55 * a);
    }
    for (name, mut text, mut color) in &mut names {
        let a = alpha_of(name.0);
        if let Some(line) = line_of(name.0) {
            let want = format!("{}:", chat_name(line.from));
            if text.0 != want {
                text.0 = want;
            }
            let base = crate::hud::player_color(&net.match_state, line.from);
            color.0 = base.with_alpha(a);
        }
    }
    for (row_text, mut text, mut color) in &mut texts {
        let a = alpha_of(row_text.0);
        if let Some(line) = line_of(row_text.0) {
            if text.0 != line.text {
                text.0.clone_from(&line.text);
            }
            color.0 = Color::srgba(0.95, 0.97, 1.0, a);
        }
    }

    if let Ok(mut node) = input_box.single_mut() {
        let display = if open { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
    if open && let Ok(mut text) = input_text.single_mut() {
        let caret = if (now * 2.0).fract() < 0.5 { "|" } else { " " };
        let want = format!("Say: {}{caret}", chat.draft);
        if text.0 != want {
            text.0 = want;
        }
    }
}

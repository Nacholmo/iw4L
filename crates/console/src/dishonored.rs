use bevy::prelude::*;
use ui::UiLayer;

use crate::{
    BindButton, ConsoleCommand, ConsoleLine, ConsoleRegistry, ConsoleSettings, ConsoleState,
    KeyBinds, display_button,
};

#[derive(Component)]
pub(crate) struct DishonoredHud;

/// What attacking would do now (assassinate, drop on a target), under the crosshair.
#[derive(Component)]
pub(crate) struct DishonoredPromptHud;

pub(crate) fn register_commands(registry: &mut ConsoleRegistry) {
    if registry.resolve("dishonored").is_none() {
        registry.register(
            crate::CommandSpec::new("dishonored")
                .usage("dishonored [on|off|status] — Corvo's movement and Blink from your Dishonored install (K toggles)"),
        );
    }
}

pub(crate) fn route(
    mut events: MessageReader<ConsoleCommand>,
    mut console: ResMut<ConsoleState>,
    settings: Res<ConsoleSettings>,
    mut line: ResMut<ConsoleLine>,
    mut mode: ResMut<frame::DishonoredMode>,
) {
    for cmd in events.read() {
        if cmd.name != "dishonored" {
            continue;
        }
        match cmd.args.first().map(String::as_str) {
            Some("on") => mode.toggle_requested = !mode.wanted,
            Some("off") => mode.toggle_requested = mode.wanted,
            None => mode.toggle_requested = true,
            Some("status") => {}
            Some(other) => {
                let msg = format!("usage: dishonored [on|off|status] (got `{other}`)");
                line.0 = msg.clone();
                console.echo(msg, settings.log_capacity);
                continue;
            }
        }
        let msg = format!(
            "dishonored active={} wanted={} ready={} {}",
            mode.active, mode.wanted, mode.ready, mode.status
        );
        diag::info!(Console, "{msg}");
        line.0 = msg.clone();
        console.echo(msg, settings.log_capacity);
    }
}

pub(crate) fn spawn_hud(commands: &mut Commands, font: Handle<Font>) {
    commands.spawn((
        DishonoredPromptHud,
        UiLayer::Overlay,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            top: percent(58),
            padding: UiRect::axes(px(10), px(4)),
            ..default()
        },
        UiTransform::from_translation(Val2::percent(-50.0, 0.0)),
        BackgroundColor(Color::srgba(0.02, 0.03, 0.04, 0.55)),
        GlobalZIndex(19_000),
        Text::new(""),
        TextFont {
            font: font.clone().into(),
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.93, 0.86)),
    ));
    commands.spawn((
        DishonoredHud,
        UiLayer::Overlay,
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            bottom: px(16),
            padding: UiRect::axes(px(10), px(4)),
            ..default()
        },
        UiTransform::from_translation(Val2::percent(-50.0, 0.0)),
        BackgroundColor(Color::srgba(0.02, 0.03, 0.04, 0.55)),
        GlobalZIndex(19_000),
        Text::new(""),
        TextFont {
            font: font.into(),
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.86, 0.90, 0.95)),
    ));
}

type HudText = (&'static mut Text, &'static mut Visibility);

/// The button bound to `+attack`, on the device the prompts are showing for.
fn attack_button(binds: &KeyBinds, devices: &frame::InputDevices) -> String {
    let Some(id) = input_iw4::command_id_lookup("+attack") else {
        return "FIRE".into();
    };
    let mut bound: Vec<BindButton> = binds
        .iter()
        .filter(|(_, b)| *b == id)
        .map(|(button, _)| button)
        .collect();
    bound.sort_by_key(|b| display_button(*b));
    match bound
        .iter()
        .find(|b| matches!(b, BindButton::Pad(_)) == devices.pad_prompts)
        .or(bound.first())
    {
        Some(BindButton::Pad(pad)) => pad.prompt(devices.style).into(),
        Some(button) => display_button(*button),
        None => "FIRE".into(),
    }
}

pub(crate) fn update_hud(
    mode: Res<frame::DishonoredMode>,
    (binds, devices): (Res<KeyBinds>, Res<frame::InputDevices>),
    mut hud: Query<HudText, (With<DishonoredHud>, Without<DishonoredPromptHud>)>,
    mut prompt: Query<HudText, With<DishonoredPromptHud>>,
) {
    for (mut text, mut vis) in &mut prompt {
        let what = match mode.prompt.filter(|_| mode.active) {
            Some(frame::DishonoredPrompt::Assassinate) => "Assassinate",
            Some(frame::DishonoredPrompt::DropAssassinate) => "Drop assassination",
            Some(frame::DishonoredPrompt::DropDive) => "Drop assassination (dive)",
            None => {
                *vis = Visibility::Hidden;
                continue;
            }
        };
        *vis = Visibility::Visible;
        let line = format!("[{}]  {what}", attack_button(&binds, &devices));
        if **text != line {
            **text = line;
        }
    }
    for (mut text, mut vis) in &mut hud {
        if mode.active {
            *vis = Visibility::Visible;
            let line = format!("DISHONORED  {}  |  K: back to MW2", mode.status);
            if **text != line {
                **text = line;
            }
        } else {
            *vis = Visibility::Hidden;
        }
    }
}

use bevy::prelude::*;
use ui::UiLayer;

use crate::{ConsoleCommand, ConsoleLine, ConsoleRegistry, ConsoleSettings, ConsoleState};

#[derive(Component)]
pub(crate) struct DishonoredHud;

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

pub(crate) fn update_hud(
    mode: Res<frame::DishonoredMode>,
    mut hud: Query<(&mut Text, &mut Visibility), With<DishonoredHud>>,
) {
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

pub mod fx;
pub mod hands;
mod pawns;
mod sound;
pub mod world;

use std::cell::RefCell;
use std::sync::{Mutex, mpsc};

use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;
use dis_data::effects::Fx;
use dis_motion::{
    BlinkEvent, BlinkMode, MeleeEvent, Motion, MotionState, MotionTuning, Reach, StepEvents,
};
use frame::{AppScreen, DishonoredDraw, DishonoredMode, DishonoredPrompt};
use glam::Affine3A;

use fx::{Attach, FxWorld};
use hands::Hands;
use pawns::{Character, Pawns, actor_of, client_of};
use world::{LadderCache, MapWorld, angles_from_iw, angles_to_iw, from_iw, surface_below, to_iw};

/// How long the view takes to settle onto a kill animation's camera.
const TAKEDOWN_EASE: f32 = 0.3;

struct Loaded {
    tuning: MotionTuning,
    npc_health: f32,
    sounds: sound::SoundBank,
    viewmodel: Option<dis_data::viewmodel::ViewModel>,
    effects: dis_data::effects::Effects,
    source: String,
}

#[derive(Default)]
struct FxState {
    marker: Option<u64>,
    fall_marker: Option<u64>,
    slide: Option<u64>,
    swim_timer: f32,
    prev_state: Option<MotionState>,
    shake: (f32, f32),
}

#[derive(Resource, Default)]
struct Dishonored {
    loading: Option<Mutex<mpsc::Receiver<Result<Loaded, String>>>>,
    tuning: Option<MotionTuning>,
    sounds: Option<sound::SoundBank>,
    hands: Option<Hands>,
    fx: Option<FxWorld>,
    motion: Option<Motion>,
    sfx: sound::Sfx,
    fx_state: FxState,
    ladders: LadderCache,
    blink_level: usize,
    time: f32,
    npc_health: f32,
    /// The view when a kill animation took it over, eased from.
    ease_from: Option<Transform>,
}

pub fn register(app: &mut App) {
    app.init_resource::<DishonoredMode>()
        .init_resource::<DishonoredDraw>()
        .init_resource::<Dishonored>()
        .add_systems(Startup, start_loading)
        .add_systems(
            Update,
            update
                .after(frame::PresentedPublished)
                .before(crate::sync_camera_from_presented)
                .before(render_scene::GfxSceneAdd)
                .in_set(frame::ClientSet::Present),
        );
}

fn start_loading(mut dis: ResMut<Dishonored>, mut mode: ResMut<DishonoredMode>) {
    let (send, receive) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("dishonored-load".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let _ = send.send(load());
        });
    match spawned {
        Ok(_) => {
            dis.loading = Some(Mutex::new(receive));
            mode.status = "reading the Dishonored install".into();
        }
        Err(e) => mode.status = format!("cannot start the Dishonored loader: {e}"),
    }
}

fn difficulty() -> dis_data::Difficulty {
    match std::env::var("IW4L_DISHONORED_DIFFICULTY").as_deref() {
        Ok("easy") => dis_data::Difficulty::Easy,
        Ok("hard") => dis_data::Difficulty::Hard,
        Ok("veryhard") => dis_data::Difficulty::VeryHard,
        _ => dis_data::Difficulty::Normal,
    }
}

fn load() -> Result<Loaded, String> {
    let start = std::time::Instant::now();
    let explicit = std::env::var_os("IW4L_DISHONORED").map(std::path::PathBuf::from);
    let install = dis_data::find_install(explicit.as_deref())
        .ok_or("Dishonored install not found (set IW4L_DISHONORED or DISHONORED_DIR)")?;
    let difficulty = difficulty();
    let data = dis_data::load(&install, difficulty)
        .map_err(|e| format!("cannot read {}: {e}", install.display()))?;
    let mut warnings = data.warnings.clone();
    let mut sounds = dis_data::sounds::load_sounds(&install, &data.blink.sound_events);
    let viewmodel = match dis_data::viewmodel::load_viewmodel(&install) {
        Ok(vm) => {
            warnings.extend(vm.warnings.iter().cloned());
            Some(vm)
        }
        Err(e) => {
            warnings.push(format!("arms not loaded: {e}"));
            None
        }
    };
    if let Some(vm) = &viewmodel {
        sounds.add_events(&install, posted_events(&data, vm));
    }
    warnings.extend(sounds.warnings.iter().cloned());
    let sounds = sound::decode(sounds, &mut warnings);
    let effects = dis_data::effects::load_effects(&install);
    warnings.extend(effects.warnings.iter().cloned());
    for w in &warnings {
        diag::warn!(World, "Dishonored: {w}");
    }
    diag::info!(
        World,
        "Dishonored install {} ({difficulty:?}) read in {}ms",
        install.display(),
        start.elapsed().as_millis()
    );
    Ok(Loaded {
        tuning: MotionTuning::from_game(&data),
        npc_health: data.melee.default_npc_health,
        sounds,
        viewmodel,
        effects,
        source: format!("{} ({difficulty:?})", install.display()),
    })
}

/// The sounds the kill and sword animations post.
fn posted_events(
    data: &dis_data::GameData,
    vm: &dis_data::viewmodel::ViewModel,
) -> std::collections::BTreeSet<String> {
    let a = &data.assassinate;
    let played: std::collections::HashSet<&str> = data
        .drop_assassinate
        .sides
        .iter()
        .chain(&a.slow)
        .chain(&a.fast)
        .chain([&a.generic])
        .map(|s| s.anim.as_str())
        .chain(
            data.melee
                .swings
                .iter()
                .flatten()
                .flat_map(|s| [Some(&s.swing), s.env_hit.as_ref(), s.env_hit_chain.as_ref()])
                .flatten()
                .map(|a| a.name.as_str()),
        )
        .collect();
    vm.sound_notifies
        .iter()
        .filter(|(clip, _)| played.contains(clip.as_str()))
        .flat_map(|(_, list)| list.iter().map(|n| n.event.clone()))
        .collect()
}

fn read_input(
    actions: &net::ClientActionInput,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    pad: Option<&Gamepad>,
) -> dis_motion::Input {
    let kb = &actions.client.kb;
    let axis = |pos: bool, neg: bool| pos as i32 as f32 - neg as i32 as f32;
    let mut move_axis = Vec2::new(
        axis(kb.moveright.active, kb.moveleft.active),
        axis(kb.forward.active, kb.back.active),
    );
    let stick = Vec2::new(actions.pad_move[1], actions.pad_move[0]);
    if stick.length() > 0.2 {
        move_axis = stick.clamp_length_max(1.0);
    }
    let bumpers = pad.map_or(0.0, |pad| {
        axis(
            pad.pressed(GamepadButton::RightTrigger),
            pad.pressed(GamepadButton::LeftTrigger),
        )
    });
    dis_motion::Input {
        move_axis,
        look: Vec2::ZERO,
        jump: kb.gostand.active,
        crouch: kb.stance.active
            || kb.movedown.active
            || kb.prone.active
            || keys.pressed(KeyCode::ControlLeft)
            || keys.pressed(KeyCode::KeyC),
        sprint: kb.sprint.active || kb.holdbreath.active,
        walk: keys.pressed(KeyCode::AltLeft),
        lean: (axis(keys.pressed(KeyCode::KeyE), keys.pressed(KeyCode::KeyQ)) + bumpers)
            .clamp(-1.0, 1.0),
        blink: kb.speed.active || mouse.pressed(MouseButton::Right) || keys.pressed(KeyCode::KeyF),
        attack: kb.attack.active,
    }
}

fn stop(
    dis: &mut Dishonored,
    mode: &mut DishonoredMode,
    draw: &mut DishonoredDraw,
    authority: Option<&mut net::AuthorityWorld>,
) {
    if let Some(authority) = authority {
        authority
            .0
            .set_external_motion(sim::ClientId(mode.client), false);
    }
    dis.motion = None;
    dis.ease_from = None;
    dis.sfx.silence();
    dis.fx_state = FxState::default();
    if let Some(fx) = dis.fx.as_mut() {
        fx.clear();
    }
    mode.active = false;
    mode.camera = None;
    mode.prompt = None;
    mode.status = "off".into();
    draw.active = false;
    draw.meshes.clear();
    draw.sprites.clear();
}

#[allow(clippy::too_many_arguments)]
fn update(
    time: Res<Time>,
    (keys, mouse): (Res<ButtonInput<KeyCode>>, Res<ButtonInput<MouseButton>>),
    (gamepads, active_pad): (Query<&Gamepad>, Option<Res<frame::ActivePad>>),
    screen: Res<AppScreen>,
    local: Res<net::LocalPresentClient>,
    presented: Res<net::PresentedSnapshot>,
    mut authority: Option<ResMut<net::AuthorityWorld>>,
    (runtime, actions, bot_notice): (
        Option<Res<audio::AudioRuntime>>,
        Res<net::ClientActionInput>,
        Option<Res<frame::BotNotice>>,
    ),
    mut mode: ResMut<DishonoredMode>,
    mut draw: ResMut<DishonoredDraw>,
    mut dis: ResMut<Dishonored>,
) {
    let dis = &mut *dis;
    let finished = dis
        .loading
        .as_ref()
        .and_then(|rx| match rx.lock().ok()?.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("the Dishonored loader stopped".into()))
            }
        });
    if let Some(result) = finished {
        dis.loading = None;
        match result {
            Ok(loaded) => {
                dis.tuning = Some(loaded.tuning);
                dis.npc_health = loaded.npc_health;
                dis.sounds = Some(loaded.sounds);
                dis.hands = loaded.viewmodel.map(Hands::new);
                dis.fx = Some(FxWorld::new(loaded.effects));
                mode.ready = true;
                mode.status = format!("ready: {}", loaded.source);
            }
            Err(e) => {
                diag::warn!(World, "Dishonored mode unavailable: {e}");
                mode.status = e;
            }
        }
    }

    let ps = presented.player(local.0).copied();
    let alive =
        authority.is_some() && *screen == AppScreen::InGame && ps.is_some_and(|p| p.pm_type == 0);
    if std::mem::take(&mut mode.toggle_requested) {
        if !mode.ready {
            diag::warn!(World, "Dishonored mode: {}", mode.status);
        } else if authority.is_none() {
            mode.status = "Dishonored mode needs this game to host the match".into();
            diag::warn!(World, "{}", mode.status);
        } else {
            mode.wanted = !mode.wanted;
        }
    }
    let leaving = mode.active && (!mode.wanted || !alive || mode.client != local.0.0);
    if leaving {
        stop(dis, &mut mode, &mut draw, authority.as_deref_mut());
        diag::info!(World, "Dishonored mode stopped");
    }
    let (Some(ps), Some(authority), Some(tuning)) =
        (ps, authority.as_deref_mut(), dis.tuning.clone())
    else {
        return;
    };
    if mode.wanted && alive && !mode.active {
        let (yaw, _) = angles_from_iw(ps.viewangles[1], ps.viewangles[0]);
        let mut motion = Motion::new(tuning.clone(), from_iw(Vec3::from_array(ps.origin)), yaw);
        motion.blink.level = dis
            .blink_level
            .min(tuning.blink.levels.len().saturating_sub(1));
        dis.motion = Some(motion);
        dis.sfx = sound::Sfx::default();
        mode.client = local.0.0;
        mode.active = true;
        authority.0.set_external_motion(local.0, true);
        diag::info!(World, "Dishonored mode started at {:?}", ps.origin);
    }
    if !mode.active {
        return;
    }
    let Some(motion) = dis.motion.as_mut() else {
        return;
    };

    let dt = time.delta_secs().min(0.1);
    dis.time += dt;
    let pad = active_pad
        .and_then(|p| p.0)
        .and_then(|e| gamepads.get(e).ok());
    let input = if mode.input_blocked {
        dis_motion::Input::default()
    } else {
        read_input(&actions, &keys, &mouse, pad)
    };
    if !mode.input_blocked {
        let tier = if keys.just_pressed(KeyCode::Digit1)
            || pad.is_some_and(|p| p.just_pressed(GamepadButton::DPadDown))
        {
            Some(0)
        } else if keys.just_pressed(KeyCode::Digit2)
            || pad.is_some_and(|p| p.just_pressed(GamepadButton::DPadUp))
        {
            Some(1)
        } else {
            None
        };
        if let Some(tier) = tier {
            dis.blink_level = tier.min(tuning.blink.levels.len().saturating_sub(1));
            motion.blink.level = dis.blink_level;
        }
    }
    // Blink travel and kills steer the view themselves until they end.
    if !matches!(motion.state, MotionState::Blinking | MotionState::Takedown) {
        let (yaw, pitch) = angles_from_iw(ps.viewangles[1], ps.viewangles[0]);
        motion.yaw = yaw;
        motion.pitch = pitch;
    }

    let characters = characters(&authority.0, local.0);
    let noticed = |client: u32| {
        bot_notice
            .as_ref()
            .and_then(|n| n.0.get(&client))
            .map(|bits| local.0.0 < 64 && bits & (1 << local.0.0) != 0)
    };
    let corvo_eye = to_iw(motion.camera.eye);
    let npc_health = dis.npc_health;
    let before = motion.pos;
    let view_before = (motion.yaw, motion.pitch);
    let ladders = RefCell::new(std::mem::take(&mut dis.ladders));
    let (events, surface, water_z) = authority.0.with_player_clip(local.0, |trace| {
        let pawns: Pawns = characters
            .iter()
            .map(|c| {
                let bot = noticed(c.client);
                let c = Character {
                    bot: bot.is_some(),
                    ..*c
                };
                let awareness = pawns::awareness(trace, &c, bot.unwrap_or(false), corvo_eye);
                (
                    actor_of(c.client),
                    pawns::pawn_info(&c, awareness, npc_health),
                )
            })
            .collect();
        let map = MapWorld {
            trace,
            ladders: &ladders,
            pawns: &pawns,
        };
        let events = motion.update(&map, &input, dt);
        let surface = surface_below(trace, motion.feet());
        let water_z = dis_motion::World::water(&map, motion.pos).map(|w| w.surface_z);
        (events, surface, water_z)
    });
    dis.ladders = ladders.into_inner();
    let moved = motion.pos - before;
    if motion.pos.z < -20000.0 || !motion.pos.is_finite() {
        mode.wanted = false;
        stop(dis, &mut mode, &mut draw, Some(authority));
        return;
    }
    let feet = to_iw(motion.feet());
    authority.0.set_origin(local.0, feet.to_array());
    let (iw_yaw, iw_pitch) = angles_to_iw(motion.yaw, motion.pitch);
    if events.drop_assassination.is_some() || events.assassination.is_some() {
        // The kill turned Corvo: carry the player's look over to where he now faces.
        let (yaw0, pitch0) = angles_to_iw(view_before.0, view_before.1);
        let turn = |a: f32, b: f32| (a - b + 540.0).rem_euclid(360.0) - 180.0;
        authority
            .0
            .turn_view(local.0, [turn(iw_pitch, pitch0), turn(iw_yaw, yaw0), 0.0]);
    }
    authority.0.set_viewangles(local.0, [iw_pitch, iw_yaw, 0.0]);
    strike(&mut authority.0, local.0, &characters, &events, npc_health);
    for b in &events.blink {
        match b {
            BlinkEvent::Released => diag::debug!(World, "Dishonored: blink"),
            BlinkEvent::Fizzled => diag::debug!(World, "Dishonored: blink fizzled"),
            _ => {}
        }
    }

    if let Some(bank) = dis.sounds.as_ref() {
        dis.sfx.update(&sound::Frame {
            bank,
            runtime: runtime.as_deref(),
            motion,
            events: &events,
            moved,
            surface,
            dt,
        });
    }
    let (shake_left, shake_strength) = dis.fx_state.shake;
    let mut meshes = Vec::new();
    let mut sprites = Vec::new();
    if let Some(fxw) = dis.fx.as_mut() {
        fxw.camera = fx::camera_basis(motion.camera.eye, motion.view_dir());
        fx_triggers(
            &mut dis.fx_state,
            fxw,
            motion,
            &tuning,
            &events,
            surface,
            water_z,
            &mut dis.sfx,
            dt,
        );
        if let Some(hands) = dis.hands.as_mut() {
            meshes = hands.update(motion, &events, dt, fxw);
            for (event, volume) in hands.posted_sounds.drain(..) {
                if let Some(bank) = dis.sounds.as_ref() {
                    dis.sfx.post(bank, runtime.as_deref(), &event, volume);
                }
            }
        }
        sprites = fxw.update(dt);
    }

    let eye = to_iw(motion.camera.eye);
    let mut angles = [iw_pitch, iw_yaw, motion.camera.roll.to_degrees()];
    if shake_left > 0.0 {
        let k = shake_strength * (shake_left / 0.35).powi(2) * 0.02;
        let t = dis.time * 40.0;
        angles[0] += (k * t.sin()).to_degrees();
        angles[1] += (k * 0.6 * (t * 1.3).cos()).to_degrees();
    }
    let fov = motion.camera.fov_deg;
    let mut camera = render_scene::transform_from_iw_view(render_scene::WorldCameraPose {
        origin: eye.to_array(),
        angles,
    });
    let offset = dis.hands.as_ref().and_then(|h| h.cam_offset);
    match (motion.takedown.run.as_ref(), offset) {
        (Some(run), Some((translation, rotation))) => {
            let from = *dis
                .ease_from
                .get_or_insert(mode.camera.map_or(camera, |(c, _)| c));
            let anim = camera.mul_transform(Transform {
                translation,
                rotation,
                ..default()
            });
            let k = (run.t / TAKEDOWN_EASE).clamp(0.0, 1.0);
            let w = k * k * (3.0 - 2.0 * k);
            camera.translation = from.translation.lerp(anim.translation, w);
            camera.rotation = from.rotation.slerp(anim.rotation, w);
        }
        _ => dis.ease_from = None,
    }
    mode.camera = Some((camera, fov));
    mode.view_angles = angles;

    let blink = motion.blink.fx;
    draw.active = true;
    draw.fov_y = fov.to_radians();
    draw.near = 0.01;
    draw.light_dir = Vec3::new(-0.35, 0.75, 0.55).normalize().to_array();
    draw.meshes = meshes;
    draw.sprites = sprites;
    draw.vignette = (blink.blur * 1.8 + blink.distortion * 0.3).clamp(0.0, 1.0);

    let state = match motion.state {
        MotionState::Walking if motion.sprinting => "Sprinting",
        MotionState::Walking if motion.crouched => "Crouched",
        MotionState::Walking => "Walking",
        MotionState::Falling => "Falling",
        MotionState::Sliding => "Sliding",
        MotionState::Mantling => "Mantling",
        MotionState::Swimming => "Swimming",
        MotionState::Ladder => "Ladder",
        MotionState::Blinking => "Blinking",
        MotionState::Takedown => "Takedown",
    };
    mode.prompt = match (motion.takedown.prompt, motion.takedown.assassinate) {
        _ if motion.takedown.diving.is_some() => None,
        (Some((_, Reach::InRange)), _) => Some(DishonoredPrompt::DropAssassinate),
        (Some((_, Reach::Track)), _) => Some(DishonoredPrompt::DropDive),
        (None, Some(_)) => Some(DishonoredPrompt::Assassinate),
        (None, None) => None,
    };
    let level = &tuning.blink.levels[motion.blink.level.min(tuning.blink.levels.len() - 1)];
    mode.status = format!(
        "{state}  {:.1} m/s  |  Blink tier {} ({:.0} m)",
        motion.speed_2d() / 100.0,
        motion.blink.level + 1,
        level.horiz_distance / 100.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn fx_triggers(
    st: &mut FxState,
    fxw: &mut FxWorld,
    m: &Motion,
    tuning: &MotionTuning,
    ev: &StepEvents,
    surface: dis_data::sounds::Surface,
    water_z: Option<f32>,
    sfx: &mut sound::Sfx,
    dt: f32,
) {
    use dis_data::sounds::Surface;
    let feet = m.feet();
    match (m.blink.mode, m.blink.target) {
        (BlinkMode::Targeting, Some(t)) => {
            let ground = t.ground_point + dis_motion::Vec3::Z * tuning.blink.target_extent[2];
            let ground_xf = fx::pitched_down(ground, m.yaw);
            let fall_xf = Affine3A::from_translation(t.point);
            match st.marker {
                Some(id) => fxw.set_transform(id, ground_xf),
                None => st.marker = Some(fxw.spawn(Fx::BlinkGround, ground_xf, Attach::World)),
            }
            match (st.fall_marker, t.point.z - ground.z > 15.0) {
                (Some(id), true) => fxw.set_transform(id, fall_xf),
                (None, true) => {
                    st.fall_marker = Some(fxw.spawn(Fx::BlinkFall, fall_xf, Attach::World))
                }
                (Some(id), false) => {
                    fxw.stop(id);
                    st.fall_marker = None;
                }
                (None, false) => {}
            }
        }
        _ => {
            for id in [st.marker.take(), st.fall_marker.take()]
                .into_iter()
                .flatten()
            {
                fxw.stop(id);
            }
        }
    }
    if ev.blink.contains(&BlinkEvent::Ended) {
        fxw.spawn(Fx::BlinkArriveLens, Affine3A::IDENTITY, Attach::Camera);
    }
    if m.state == MotionState::Sliding {
        let xf = fx::placed(feet, m.vel.y.atan2(m.vel.x));
        match st.slide {
            Some(id) => fxw.set_transform(id, xf),
            None => {
                let kind = if surface == Surface::Stone {
                    Fx::SlideStone
                } else {
                    Fx::SlideGeneric
                };
                st.slide = Some(fxw.spawn(kind, xf, Attach::World));
            }
        }
    } else if let Some(id) = st.slide.take() {
        fxw.stop(id);
    }
    for (surface, at) in std::mem::take(&mut sfx.steps) {
        let kind = match surface {
            Surface::Gravel => Fx::StepGravel,
            Surface::Water => Fx::StepWater,
            _ => continue,
        };
        fxw.spawn(kind, fx::placed(at, m.yaw), Attach::World);
    }
    if let Some(impact) = ev.landed {
        if impact > 900.0 && matches!(surface, Surface::Gravel | Surface::Stone) {
            fxw.spawn(Fx::LandDirt, fx::placed(feet, m.yaw), Attach::World);
        }
        if impact > 900.0 {
            st.shake = (0.35, (impact / tuning.fall_damage_speed).clamp(0.3, 1.5));
        }
    }
    if m.state == MotionState::Swimming {
        if st.prev_state != Some(MotionState::Swimming)
            && let Some(z) = water_z
        {
            fxw.spawn(
                Fx::WaterSplash,
                fx::placed(dis_motion::Vec3::new(m.pos.x, m.pos.y, z), m.yaw),
                Attach::World,
            );
        }
        st.swim_timer -= dt;
        if st.swim_timer <= 0.0
            && m.speed_2d() > 30.0
            && let Some(z) = water_z
        {
            st.swim_timer = 0.45;
            fxw.spawn(
                Fx::Swimming,
                fx::placed(dis_motion::Vec3::new(m.pos.x, m.pos.y, z), m.yaw),
                Attach::World,
            );
        }
    } else if st.prev_state == Some(MotionState::Swimming) {
        fxw.spawn(Fx::CameraWater, Affine3A::IDENTITY, Attach::Camera);
    }
    // The blade striking the world shakes the view, scaled from the game's shake strength.
    if let Some(MeleeEvent::EnvHit { shake, .. }) = ev
        .melee
        .iter()
        .find(|e| matches!(e, MeleeEvent::EnvHit { .. }))
    {
        st.shake = (0.25, shake / 400.0);
    }
    st.prev_state = Some(m.state);
    st.shake.0 = (st.shake.0 - dt).max(0.0);
}

/// The other living players, as the sword and the kills see them.
fn characters(world: &sim::SimWorld, local: sim::ClientId) -> Vec<Character> {
    use playerstate_iw4::{PM_TYPE_LAST_STAND, PM_TYPE_NORMAL_LINKED, pm_flags};
    let mut out = Vec::new();
    world.visit_players(|id, ps| {
        let alive = matches!(ps.pm_type, 0 | PM_TYPE_NORMAL_LINKED | PM_TYPE_LAST_STAND);
        if id != local && alive && ps.health > 0 {
            out.push(Character {
                client: id.0,
                bot: false,
                origin: ps.origin,
                eye_height: ps.view_height_current,
                angles: ps.viewangles,
                health: ps.health,
                max_health: ps.max_health,
                sprinting: ps.pm_flags & pm_flags::SPRINTING != 0,
            });
        }
    });
    out
}

/// Lands this frame's sword hits and kills on the match: the host's sim applies them as melee
/// damage from the local player on its next tick, placing a paired kill's victim first.
fn strike(
    world: &mut sim::SimWorld,
    local: sim::ClientId,
    characters: &[Character],
    ev: &StepEvents,
    npc_health: f32,
) {
    let mut queue = |actor: u32, blow: sim::Blow, place: Option<([f32; 3], f32)>| {
        let Some(target) = client_of(actor) else {
            return;
        };
        if !characters.iter().any(|c| c.client == target) {
            return;
        }
        world.strike(sim::ExternalStrike {
            attacker: local,
            target: sim::ClientId(target),
            blow,
            place,
        });
    };
    for e in &ev.melee {
        let MeleeEvent::Hit {
            target,
            damage,
            killing,
        } = e
        else {
            continue;
        };
        let max = client_of(*target)
            .and_then(|t| characters.iter().find(|c| c.client == t))
            .map_or(100, |c| c.max_health);
        let blow = if *killing {
            sim::Blow::Kill
        } else {
            sim::Blow::Damage(pawns::mw2_damage(*damage, max, npc_health))
        };
        diag::debug!(World, "Dishonored: sword hit actor {target} ({blow:?})");
        queue(*target, blow, None);
    }
    if let Some((target, side)) = ev.drop_assassination {
        diag::info!(
            World,
            "Dishonored: drop assassination of actor {target} from the {side:?}"
        );
        queue(target, sim::Blow::Kill, None);
    }
    if let Some(a) = ev.assassination {
        let place = (!a.generic).then(|| {
            let (yaw, _) = angles_to_iw(a.victim_yaw, 0.0);
            (to_iw(a.victim_feet).to_array(), yaw)
        });
        diag::info!(
            World,
            "Dishonored: assassination of actor {} from the {:?} (fast={} generic={})",
            a.target,
            a.side,
            a.fast,
            a.generic
        );
        queue(a.target, sim::Blow::Kill, place);
    }
}

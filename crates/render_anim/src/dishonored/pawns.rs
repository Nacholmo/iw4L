use std::collections::HashMap;

use bevy::prelude::Vec3;
use dis_motion::{Awareness, PawnInfo};
use movement_iw4::GroundTraceInput;
use trace_iw4::Trace;

use super::world::{CM_PER_INCH, from_iw};

/// A body is a fixed box (`sim::PLAYER_MAXS`) whatever the stance, so the torso sits where a
/// box character's does.
const BODY_HEIGHT: f32 = sim::PLAYER_MAXS[2];
const TORSO: f32 = BODY_HEIGHT * dis_motion::boxworld::BOX_TORSO_HEIGHT;
/// How far off a human's view Corvo can stand and still be seen (half-angle cosine, 60°).
const NOTICE_CONE_COS: f32 = 0.5;
const SIGHT_MASK: u32 = sim::MASK_SHOT & !sim::bullet_collision::CONTENTS_BODY;

/// An MW2 player as Corvo's sword and kills see it, in IW4 map units.
#[derive(Clone, Copy, Debug)]
pub struct Character {
    pub client: u32,
    pub bot: bool,
    pub origin: [f32; 3],
    pub eye_height: f32,
    /// View angles, degrees (pitch, yaw, roll).
    pub angles: [f32; 3],
    pub health: i32,
    pub max_health: i32,
    pub sprinting: bool,
}

/// Dishonored's character health and MW2's are different scales: a blow is worth the same share
/// of either, so the game's three blows still kill whatever the MW2 health.
pub fn dishonored_health(health: i32, max_health: i32, npc_health: f32) -> f32 {
    health.max(0) as f32 * npc_health / max_health.max(1) as f32
}

pub fn mw2_damage(damage: f32, max_health: i32, npc_health: f32) -> i32 {
    (damage * max_health.max(1) as f32 / npc_health.max(1.0)).ceil() as i32
}

pub fn actor_of(client: u32) -> u32 {
    client + 1
}

pub fn client_of(actor: u32) -> Option<u32> {
    actor.checked_sub(1)
}

/// Whether a human faces `target` (within the cone) with nothing solid between.
fn human_sees(
    trace: &dyn Fn(GroundTraceInput) -> Trace,
    c: &Character,
    eye: Vec3,
    target: Vec3,
) -> bool {
    let to = target - eye;
    let (pitch, yaw) = (c.angles[0].to_radians(), c.angles[1].to_radians());
    let look = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    );
    if look.dot(to.normalize_or_zero()) < NOTICE_CONE_COS {
        return false;
    }
    trace(GroundTraceInput {
        start: eye.to_array(),
        end: target.to_array(),
        mins: [0.0; 3],
        maxs: [0.0; 3],
        tracemask: SIGHT_MASK,
    })
    .fraction
        >= 1.0
}

/// Bots have noticed Corvo when they see him or are fighting him; humans when they look at him.
/// Either way they fight, and the rest can be assassinated.
pub fn awareness(
    trace: &dyn Fn(GroundTraceInput) -> Trace,
    c: &Character,
    bot_noticed: bool,
    corvo_eye: Vec3,
) -> Awareness {
    let eye = Vec3::from_array(c.origin) + Vec3::Z * c.eye_height;
    let noticed = if c.bot {
        bot_noticed
    } else {
        human_sees(trace, c, eye, corvo_eye)
    };
    if noticed {
        Awareness::InCombat
    } else {
        Awareness::Unaware
    }
}

pub fn pawn_info(c: &Character, awareness: Awareness, npc_health: f32) -> PawnInfo {
    let feet = from_iw(Vec3::from_array(c.origin));
    PawnInfo {
        center: feet + dis_motion::Vec3::Z * (BODY_HEIGHT * 0.5 * CM_PER_INCH),
        floor_z: feet.z,
        torso_z: feet.z + TORSO * CM_PER_INCH,
        yaw: -c.angles[1].to_radians(),
        health: dishonored_health(c.health, c.max_health, npc_health),
        awareness,
        running: c.sprinting,
    }
}

pub type Pawns = HashMap<u32, PawnInfo>;

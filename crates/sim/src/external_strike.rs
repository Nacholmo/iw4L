//! Blows struck by a player whose motion an outside controller drives (Dishonored mode's sword
//! and assassinations). The host queues them on its own world; the tick lands them as melee
//! damage before players move, so a victim placed for a paired kill dies where it was put.

use crate::frame::FrameWorld;
use crate::match_state::{ClientLifecycle, EventAudience};
use crate::world::{ClientId, Tick};
use crate::{DamageAttempt, DamageSource, PelletId};

/// `torso_upper`: melee ignores location scaling, so this only names where the blood goes.
const HITLOC_TORSO_UPPER: u8 = 4;
/// A kill deals this many times the target's health, so damage-reducing perks still let it through.
const KILL_DAMAGE_FACTOR: i32 = 10;
const TORSO_HEIGHT: f32 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Blow {
    Damage(i32),
    Kill,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExternalStrike {
    pub attacker: ClientId,
    pub target: ClientId,
    pub blow: Blow,
    /// Where the target's feet go and which way it faces (yaw, degrees) before the blow lands.
    pub place: Option<([f32; 3], f32)>,
}

fn alive(world: &FrameWorld, id: ClientId) -> bool {
    world
        .client_meta(id)
        .is_some_and(|m| m.lifecycle == ClientLifecycle::Alive)
}

/// Lands one strike. Only a living attacker under external motion may strike, and only another
/// living player.
pub(crate) fn apply(world: &mut FrameWorld, tick: Tick, strike: &ExternalStrike) -> bool {
    let ExternalStrike {
        attacker, target, ..
    } = *strike;
    if attacker == target
        || !world.external_motion.contains(&attacker)
        || !alive(world, attacker)
        || !alive(world, target)
    {
        return false;
    }
    if let Some((feet, yaw)) = strike.place {
        world.set_origin(target, feet);
        world.set_viewangles(target, [0.0, yaw, 0.0]);
    }
    let (Some(from), Some(victim)) = (
        world.player(attacker).copied(),
        world.player(target).copied(),
    ) else {
        return false;
    };
    let amount = match strike.blow {
        Blow::Damage(amount) => amount,
        Blow::Kill => victim.health.max(victim.max_health).max(1) * KILL_DAMAGE_FACTOR,
    };
    let (Some(attacker_life), Some(target_life)) = (
        world.client_meta(attacker).map(|m| m.life_sequence),
        world.client_meta(target).map(|m| m.life_sequence),
    ) else {
        return false;
    };
    let eye = [
        from.origin[0],
        from.origin[1],
        from.origin[2] + from.view_height_current,
    ];
    let attempt = DamageAttempt {
        splash: false,
        source: DamageSource::Melee,
        pellet: PelletId(0),
        attacker,
        attacker_life,
        target,
        target_life,
        weapon: from.weapon,
        amount,
        killcam_entity_start_time: 0,
        inflictor_origin: Some(eye),
        hitloc: HITLOC_TORSO_UPPER,
    };
    let landed = !matches!(
        crate::damage::apply_damage_attempt(world, tick, &attempt),
        crate::DamageOutcome::Refused(_)
    );
    if landed {
        let torso = [
            victim.origin[0],
            victim.origin[1],
            victim.origin[2] + TORSO_HEIGHT,
        ];
        let to = [0, 1, 2].map(|i| torso[i] - eye[i]);
        let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        world.push_entity_event(
            tick,
            EventAudience::All,
            entity_iw4::EntityEventKind::MELEE_BLOOD,
            crate::EntityEventPayload {
                number: attacker.0 as i32,
                attacker_entity_num: attacker.0 as i32,
                other_entity_num: target.0 as i32,
                weapon: from.weapon,
                origin: torso,
                direction: if len > 0.0 { to.map(|c| c / len) } else { to },
                ..Default::default()
            },
        );
    }
    landed
}

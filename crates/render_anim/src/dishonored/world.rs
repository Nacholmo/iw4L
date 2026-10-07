use std::cell::RefCell;
use std::collections::HashMap;

use bevy::prelude::Vec3;
use dis_motion::{Hit, Ladder, Water};
use movement_iw4::GroundTraceInput;
use trace_iw4::{HITTYPE_ENTITY, Trace};

pub const CM_PER_INCH: f32 = 2.54;

const MASK_PLAYER_SOLID: u32 = sim::MASK_PLAYER_SOLID;
const CONTENTS_WATER: u32 = 0x20;
const SURF_LADDER: u32 = movement_iw4::SURF_LADDER;
const MAX_CLIENTS: u16 = 18;
const LADDER_REACH: f32 = 1024.0;
const LADDER_STEP: f32 = 8.0;

pub fn to_iw(v: dis_motion::Vec3) -> Vec3 {
    Vec3::new(v.x, -v.y, v.z) / CM_PER_INCH
}

pub fn from_iw(v: Vec3) -> dis_motion::Vec3 {
    dis_motion::Vec3::new(v.x, -v.y, v.z) * CM_PER_INCH
}

pub fn angles_to_iw(yaw: f32, pitch: f32) -> (f32, f32) {
    (-yaw.to_degrees(), -pitch.to_degrees())
}

pub fn angles_from_iw(yaw_deg: f32, pitch_deg: f32) -> (f32, f32) {
    (-yaw_deg.to_radians(), -pitch_deg.to_radians())
}

#[derive(Default)]
pub struct LadderCache(HashMap<[i32; 4], (f32, f32)>);

pub struct MapWorld<'a> {
    pub trace: &'a dyn Fn(GroundTraceInput) -> Trace,
    pub ladders: &'a RefCell<LadderCache>,
}

impl MapWorld<'_> {
    fn trace_iw(&self, start: Vec3, end: Vec3, half: Vec3, mask: u32) -> Trace {
        let half = half.abs();
        (self.trace)(GroundTraceInput {
            start: start.to_array(),
            end: end.to_array(),
            mins: (-half).to_array(),
            maxs: half.to_array(),
            tracemask: mask,
        })
    }

    fn line_hits_ladder(&self, from: Vec3, dir: Vec3, reach: f32) -> Option<Trace> {
        let t = self.trace_iw(from, from + dir * reach, Vec3::ZERO, MASK_PLAYER_SOLID);
        (t.fraction < 1.0 && t.surface_flags & SURF_LADDER != 0).then_some(t)
    }

    fn ladder_span(&self, at: Vec3, dir: Vec3, reach: f32) -> (f32, f32) {
        let (mut bottom, mut top) = (at.z, at.z);
        while top - at.z < LADDER_REACH
            && self
                .line_hits_ladder(Vec3::new(at.x, at.y, top + LADDER_STEP), dir, reach)
                .is_some()
        {
            top += LADDER_STEP;
        }
        while at.z - bottom < LADDER_REACH
            && self
                .line_hits_ladder(Vec3::new(at.x, at.y, bottom - LADDER_STEP), dir, reach)
                .is_some()
        {
            bottom -= LADDER_STEP;
        }
        (bottom, top)
    }
}

impl dis_motion::World for MapWorld<'_> {
    fn sweep(
        &self,
        start: dis_motion::Vec3,
        end: dis_motion::Vec3,
        half: dis_motion::Vec3,
    ) -> Option<Hit> {
        let (s, e) = (to_iw(start), to_iw(end));
        let t = self.trace_iw(
            s,
            e,
            Vec3::from(half.to_array()) / CM_PER_INCH,
            MASK_PLAYER_SOLID,
        );
        let start_penetrating = t.startsolid != 0 || t.allsolid != 0;
        if t.fraction >= 1.0 && !start_penetrating {
            return None;
        }
        let time = t.fraction.clamp(0.0, 1.0);
        let n = Vec3::from_array(t.normal);
        let entity = t.hit_type == HITTYPE_ENTITY;
        Some(Hit {
            time,
            location: start + (end - start) * time,
            normal: dis_motion::Vec3::new(n.x, -n.y, n.z),
            start_penetrating,
            actor: if entity { u32::from(t.hit_id) + 1 } else { 0 },
            is_pawn: entity && t.hit_id < MAX_CLIENTS,
        })
    }

    fn water(&self, point: dis_motion::Vec3) -> Option<Water> {
        let p = to_iw(point);
        let inside = self.trace_iw(p, p, Vec3::ZERO, CONTENTS_WATER);
        if inside.startsolid == 0 && inside.allsolid == 0 {
            return None;
        }
        let above = p + Vec3::Z * 512.0;
        let down = self.trace_iw(above, p, Vec3::ZERO, CONTENTS_WATER);
        let surface = if down.startsolid == 0 && down.fraction < 1.0 {
            above.z + (p.z - above.z) * down.fraction
        } else {
            above.z
        };
        Some(Water {
            surface_z: surface * CM_PER_INCH,
        })
    }

    fn ladder(&self, center: dis_motion::Vec3, half: dis_motion::Vec3) -> Option<Ladder> {
        let c = to_iw(center);
        let reach = half.x.max(half.y) / CM_PER_INCH + 4.0;
        for dir in [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y] {
            let Some(hit) = self.line_hits_ladder(c, dir, reach) else {
                continue;
            };
            let n = Vec3::from_array(hit.normal);
            let flat = Vec3::new(n.x, n.y, 0.0).normalize_or_zero();
            if flat == Vec3::ZERO {
                continue;
            }
            let key = [
                (n.x * 64.0) as i32,
                (n.y * 64.0) as i32,
                (hit.endpos[0] * flat.x + hit.endpos[1] * flat.y) as i32,
                ((hit.endpos[0] * -flat.y + hit.endpos[1] * flat.x) / 64.0) as i32,
            ];
            let cached = self.ladders.borrow().0.get(&key).copied();
            let (bottom, top) = cached.unwrap_or_else(|| {
                let span = self.ladder_span(c, -flat, reach);
                self.ladders.borrow_mut().0.insert(key, span);
                span
            });
            return Some(Ladder {
                normal: dis_motion::Vec3::new(flat.x, -flat.y, 0.0),
                bottom_z: bottom * CM_PER_INCH,
                top_z: top * CM_PER_INCH,
            });
        }
        None
    }
}

pub fn surface_below(
    trace: &dyn Fn(GroundTraceInput) -> Trace,
    feet: dis_motion::Vec3,
) -> dis_data::sounds::Surface {
    use dis_data::sounds::Surface;
    let p = to_iw(feet);
    let water = trace(GroundTraceInput {
        start: (p + Vec3::Z * 2.0).to_array(),
        end: (p + Vec3::Z * 2.0).to_array(),
        mins: [0.0; 3],
        maxs: [0.0; 3],
        tracemask: CONTENTS_WATER,
    });
    if water.startsolid != 0 || water.allsolid != 0 {
        return Surface::Water;
    }
    let t = trace(GroundTraceInput {
        start: (p + Vec3::Z * 4.0).to_array(),
        end: (p - Vec3::Z * 16.0).to_array(),
        mins: [0.0; 3],
        maxs: [0.0; 3],
        tracemask: MASK_PLAYER_SOLID,
    });
    if t.fraction >= 1.0 {
        return Surface::Stone;
    }
    match movement_iw4::SURFACE_TYPE_NAMES
        .get((t.surface_flags >> 20) as usize & 0x1f)
        .copied()
        .unwrap_or("default")
    {
        "metal" | "paintedmetal" => Surface::Metal,
        "wood" | "bark" => Surface::Wood,
        "dirt" | "gravel" | "sand" | "mud" | "grass" | "foliage" | "snow" | "slush" => {
            Surface::Gravel
        }
        "water" => Surface::Water,
        "ceramic" => Surface::Rooftile,
        _ => Surface::Stone,
    }
}

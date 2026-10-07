use std::sync::Arc;

use dis_data::viewmodel::{MeshPart, ParticleNotify, ViewModel};
use dis_motion::{BlinkMode, Motion, MotionState};
use edge_anim::Joint;
use frame::{DishonoredMesh, DishonoredTexture};
use glam::{Mat4, Quat, Vec3};

use super::fx::{Attach, FxWorld};

const SCALE: f32 = 0.01;
const BLEND: f32 = 0.18;

#[derive(Clone, PartialEq)]
struct Track {
    name: String,
    looping: bool,
}

#[derive(Default)]
struct Layer {
    current: Option<Track>,
    time: f32,
    previous: Option<(Track, f32)>,
    fade: f32,
}

impl Layer {
    fn play(&mut self, name: &str, looping: bool) {
        let t = Track {
            name: name.to_string(),
            looping,
        };
        if self.current.as_ref() == Some(&t) {
            return;
        }
        if let Some(cur) = self.current.take() {
            self.previous = Some((cur, self.time));
            self.fade = BLEND;
        }
        self.current = Some(t);
        self.time = 0.0;
    }

    fn advance(&mut self, dt: f32, vm: &ViewModel) -> Vec<ParticleNotify> {
        let mut fired = Vec::new();
        if let Some(t) = &self.current
            && let (Some(list), Some(a)) =
                (vm.particle_notifies.get(&t.name), vm.anims.get(&t.name))
        {
            let (from, to) = (self.time, self.time + dt);
            for n in list {
                let hit = if t.looping && a.duration > 0.0 {
                    let k = ((from - n.time) / a.duration).ceil();
                    n.time + k * a.duration < to
                } else {
                    n.time >= from && n.time < to && n.time <= a.duration
                };
                if hit {
                    fired.push(n.clone());
                }
            }
        }
        self.time += dt;
        self.fade = (self.fade - dt).max(0.0);
        if let Some((_, t)) = self.previous.as_mut() {
            *t += dt;
        }
        if self.fade <= 0.0 {
            self.previous = None;
        }
        fired
    }

    fn done(&self, vm: &ViewModel) -> bool {
        match &self.current {
            Some(t) if !t.looping => vm
                .anims
                .get(&t.name)
                .is_none_or(|a| self.time >= a.duration),
            _ => false,
        }
    }
}

struct Part {
    texture: Option<Arc<DishonoredTexture>>,
    indices: Arc<Vec<u32>>,
}

pub struct Hands {
    vm: ViewModel,
    world_bind_inv: Vec<Mat4>,
    cam_joint: usize,
    attach_joint: usize,
    base: Layer,
    left: Layer,
    left_joints: Vec<bool>,
    arms: Part,
    sword: Option<Part>,
    prev_state: MotionState,
    prev_blink: BlinkMode,
    landing: f32,
    sword_socket: Mat4,
    attached_fx: Vec<(u64, usize, Mat4)>,
}

pub fn texture_with_mips(width: u32, height: u32, rgba: &[u8], srgb: bool) -> DishonoredTexture {
    let mut levels = vec![rgba.to_vec()];
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let level = levels.last().expect("first level");
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: u32, yy: u32| {
                        u32::from(level[((yy.min(h - 1) * w + xx.min(w - 1)) * 4 + c) as usize])
                    };
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        levels.push(next);
        (w, h) = (nw, nh);
    }
    DishonoredTexture {
        width,
        height,
        srgb,
        levels,
    }
}

fn part(p: &MeshPart) -> Part {
    Part {
        texture: p
            .diffuse
            .as_ref()
            .map(|t| Arc::new(texture_with_mips(t.width, t.height, &t.pixels, true))),
        indices: Arc::new(p.mesh.indices.clone()),
    }
}

fn world_pose(bones: &[upk::skelmesh::Bone], local: impl Fn(usize) -> Mat4) -> Vec<Mat4> {
    let mut w: Vec<Mat4> = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        let l = local(i);
        w.push(if i == 0 || b.parent == i {
            l
        } else {
            w[b.parent] * l
        });
    }
    w
}

// Pitch and roll are negated by Unreal's left-handed axes.
fn rotator(r: [i32; 3]) -> Mat4 {
    let a = |v: i32| v as f32 / 65536.0 * std::f32::consts::TAU;
    Mat4::from_quat(
        Quat::from_rotation_z(a(r[1]))
            * Quat::from_rotation_y(-a(r[0]))
            * Quat::from_rotation_x(-a(r[2])),
    )
}

fn sample(vm: &ViewModel, track: &Track, time: f32) -> Option<Vec<Joint>> {
    let a = vm.anims.get(&track.name)?;
    let t = if track.looping && a.duration > 0.0 {
        time.rem_euclid(a.duration)
    } else {
        time.min(a.duration)
    };
    a.evaluate(&vm.skeleton, t, false).ok()
}

fn blend(a: &mut [Joint], b: &[Joint], w: f32, mask: Option<&[bool]>) {
    for (i, (ja, jb)) in a.iter_mut().zip(b).enumerate() {
        if mask.is_some_and(|m| !m[i]) {
            continue;
        }
        let rb = if ja.rotation.dot(jb.rotation) < 0.0 {
            -jb.rotation
        } else {
            jb.rotation
        };
        ja.rotation = ja.rotation.slerp(rb, w);
        ja.translation = ja.translation.lerp(jb.translation, w);
    }
}

fn layer_pose(vm: &ViewModel, l: &Layer) -> Option<Vec<Joint>> {
    let cur = l.current.as_ref()?;
    let mut pose = sample(vm, cur, l.time)?;
    if let Some((prev, t)) = &l.previous
        && let Some(p) = sample(vm, prev, *t)
    {
        let w = (l.fade / BLEND).clamp(0.0, 1.0);
        let mut out = p;
        blend(&mut out, &pose, 1.0 - w, None);
        pose = out;
    }
    Some(pose)
}

impl Hands {
    pub fn new(vm: ViewModel) -> Self {
        let sword_origin = vm.sword.as_ref().map_or(Mat4::IDENTITY, |s| {
            rotator(s.mesh.rot_origin) * Mat4::from_translation(-Vec3::from(s.mesh.mesh_origin))
        });
        let bones = &vm.arms.mesh.bones;
        let find = |n: &str| {
            bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(n))
                .unwrap_or(0)
        };
        let bind = world_pose(bones, |i| {
            let b = &bones[i];
            Mat4::from_rotation_translation(Quat::from_array(b.orientation), Vec3::from(b.position))
        });
        let mut left_joints = vec![false; bones.len()];
        if let Some(a) = vm.anims.get("Powers_Idle") {
            for h in &a.joint_hashes {
                if let Some(j) = vm.skeleton.joint_by_hash(*h) {
                    left_joints[j] = bones[j].name != "root0_jnt";
                }
            }
        }
        Self {
            world_bind_inv: bind.iter().map(|m| m.inverse()).collect(),
            cam_joint: find("camera_jnt"),
            attach_joint: find("handAttachment_R_jnt"),
            base: Layer::default(),
            left: Layer::default(),
            left_joints,
            arms: part(&vm.arms),
            sword: vm.sword.as_ref().map(part),
            prev_state: MotionState::Walking,
            prev_blink: BlinkMode::Idle,
            landing: 0.0,
            sword_socket: rotator(vm.sword_socket.unwrap_or([0, 16384, 0])) * sword_origin,
            attached_fx: Vec::new(),
            vm,
        }
    }

    pub fn update(&mut self, m: &Motion, dt: f32, fx: &mut FxWorld) -> Vec<DishonoredMesh> {
        let speed = m.speed_2d();
        let moving = speed > 40.0;
        if m.state == MotionState::Walking
            && matches!(
                self.prev_state,
                MotionState::Falling | MotionState::Blinking
            )
        {
            self.landing = 0.5;
        }
        self.landing = (self.landing - dt).max(0.0);
        let (base, looping) = match m.state {
            MotionState::Mantling => (
                match m.last_mantle {
                    Some(dis_motion::MantleKind::Low) => "Sword_Ready_MantleLow",
                    Some(dis_motion::MantleKind::Medium) => "Sword_Ready_MantleMedium",
                    _ => "Sword_Ready_MantleHigh",
                },
                false,
            ),
            MotionState::Sliding => ("Sword_SlideLoop", true),
            MotionState::Swimming => (
                if moving {
                    "Empty_SwimN"
                } else {
                    "Empty_SwimIdle"
                },
                true,
            ),
            MotionState::Falling | MotionState::Ladder => ("Sword_Ready_Jump", false),
            MotionState::Blinking => ("Sword_Ready_Idle", true),
            MotionState::Walking if self.landing > 0.0 => ("Sword_Ready_JumpLandSmall", false),
            MotionState::Walking => match (m.crouched, moving, m.sprinting, speed < 250.0) {
                (true, false, _, _) => ("Sword_Sneak_Idle", true),
                (true, true, _, _) => ("Sword_Sneak_Walk", true),
                (false, false, _, _) => ("Sword_Ready_Idle", true),
                (false, true, true, _) => ("Sword_Ready_Sprint", true),
                (false, true, false, true) => ("Sword_Ready_Walk", true),
                (false, true, false, false) => ("Sword_Ready_Run", true),
            },
        };
        self.base.play(base, looping);

        let blink = m.blink.mode;
        let left_name = match blink {
            BlinkMode::Targeting => {
                if self.prev_blink != BlinkMode::Targeting {
                    self.left.play("Powers_Cast_Blink_In", false);
                }
                self.left
                    .done(&self.vm)
                    .then_some(("Powers_Cast_Blink_Loop", true))
            }
            BlinkMode::Travelling => Some(
                if self
                    .vm
                    .anims
                    .contains_key("Generic_Powers_Cast_Blink_Travel")
                {
                    ("Generic_Powers_Cast_Blink_Travel", false)
                } else {
                    ("Powers_Cast_Blink_Out", false)
                },
            ),
            BlinkMode::Cooldown | BlinkMode::AbortCooldown if !self.left.done(&self.vm) => None,
            _ => Some(match m.state {
                MotionState::Falling => ("Powers_Jump", false),
                MotionState::Walking if m.sprinting && moving => ("Powers_Sprint", true),
                MotionState::Walking if moving => ("Powers_Walk", true),
                _ => ("Powers_Idle", true),
            }),
        };
        if blink == BlinkMode::Cooldown && self.prev_blink == BlinkMode::Travelling {
            self.left.play("Powers_Cast_Blink_Out", false);
        } else if let Some((n, l)) = left_name {
            self.left.play(n, l);
        }
        self.prev_blink = blink;
        self.prev_state = m.state;
        let mut fired = self.base.advance(dt, &self.vm);
        fired.extend(self.left.advance(dt, &self.vm));

        let Some(mut pose) = layer_pose(&self.vm, &self.base) else {
            return Vec::new();
        };
        let left_on = !matches!(
            m.state,
            MotionState::Mantling | MotionState::Sliding | MotionState::Swimming
        );
        if left_on {
            if let Some(lp) = layer_pose(&self.vm, &self.left) {
                blend(&mut pose, &lp, 1.0, Some(&self.left_joints));
            }
        } else {
            fired.retain(|n| n.socket.as_deref() != Some("Tattoo"));
        }
        let bones = &self.vm.arms.mesh.bones;
        let world = world_pose(bones, |i| {
            Mat4::from_rotation_translation(pose[i].rotation, pose[i].translation)
        });
        let to_view = Mat4::from_cols(
            Vec3::new(0.0, SCALE, 0.0).extend(0.0),
            Vec3::new(SCALE, 0.0, 0.0).extend(0.0),
            Vec3::new(0.0, 0.0, SCALE).extend(0.0),
            glam::Vec4::W,
        );
        let view = to_view * world[self.cam_joint].inverse();

        let cam_to_fx = Mat4::from_cols(glam::Vec4::Z, glam::Vec4::X, glam::Vec4::Y, glam::Vec4::W)
            * world[self.cam_joint].inverse();
        let as_affine = glam::Affine3A::from_mat4;
        for n in fired {
            let (bone, offset) = match n.socket.as_ref().and_then(|s| self.vm.sockets.get(s)) {
                Some((b, loc, rot)) => (
                    b.clone(),
                    Mat4::from_translation(Vec3::from(*loc)) * rotator(*rot),
                ),
                None => (n.bone.clone().unwrap_or_else(String::new), Mat4::IDENTITY),
            };
            let Some(bi) = bones
                .iter()
                .position(|b| b.name.eq_ignore_ascii_case(&bone))
            else {
                continue;
            };
            let id = fx.spawn_def(
                n.system.clone(),
                as_affine(cam_to_fx * world[bi] * offset),
                Attach::Viewmodel,
            );
            if n.attached && id != 0 {
                self.attached_fx.push((id, bi, offset));
            }
        }
        self.attached_fx.retain(|(id, ..)| fx.is_live(*id));
        for (id, bi, offset) in &self.attached_fx {
            fx.set_transform(*id, as_affine(cam_to_fx * world[*bi] * *offset));
        }

        let skin = |part: &MeshPart, mats: &[Mat4], rigid: Option<Mat4>| -> Vec<[f32; 8]> {
            part.mesh
                .vertices
                .iter()
                .map(|v| {
                    let m = rigid.unwrap_or_else(|| {
                        let mut acc = Mat4::ZERO;
                        for k in 0..4 {
                            if v.weights[k] > 0.0 {
                                acc += mats[v.bones[k] as usize] * v.weights[k];
                            }
                        }
                        acc
                    });
                    let p = m.transform_point3(Vec3::from(v.position));
                    let n = m
                        .transform_vector3(Vec3::from(v.normal))
                        .normalize_or_zero();
                    [p.x, p.y, p.z, n.x, n.y, n.z, v.uv[0], v.uv[1]]
                })
                .collect()
        };
        let skin_mats: Vec<Mat4> = world
            .iter()
            .zip(&self.world_bind_inv)
            .map(|(w, ib)| view * *w * *ib)
            .collect();
        let mut out = vec![DishonoredMesh {
            texture: self.arms.texture.clone(),
            vertices: skin(&self.vm.arms, &skin_mats, None),
            indices: self.arms.indices.clone(),
        }];
        let unarmed = self
            .base
            .current
            .as_ref()
            .is_some_and(|t| t.name.starts_with("Empty_"));
        if let (Some(mesh), Some(part)) = (self.vm.sword.as_ref(), self.sword.as_ref())
            && !unarmed
        {
            let attach = view * world[self.attach_joint] * self.sword_socket;
            out.push(DishonoredMesh {
                texture: part.texture.clone(),
                vertices: skin(mesh, &[], Some(attach)),
                indices: part.indices.clone(),
            });
        }
        out
    }
}

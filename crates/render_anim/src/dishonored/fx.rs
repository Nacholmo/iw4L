use std::collections::HashMap;
use std::sync::Arc;

use cascade::{Alignment, Blend, Instance, SystemDef};
use dis_data::effects::{Effects, Fx};
use dis_motion::Vec3 as UVec3;
use frame::{DishonoredSprites, DishonoredTexture};
use glam::{Affine3A, Mat3};

use super::world::to_iw;

const VIEW_SCALE: f32 = 0.01;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    World,
    Camera,
    Viewmodel,
}

struct Live {
    id: u64,
    inst: Instance,
    attach: Attach,
}

pub struct FxWorld {
    effects: Effects,
    live: Vec<Live>,
    next_id: u64,
    textures: HashMap<String, Arc<DishonoredTexture>>,
    pub camera: Affine3A,
}

impl FxWorld {
    pub fn new(effects: Effects) -> Self {
        Self {
            effects,
            live: Vec::new(),
            next_id: 1,
            textures: HashMap::new(),
            camera: Affine3A::IDENTITY,
        }
    }

    pub fn spawn(&mut self, fx: Fx, transform: Affine3A, attach: Attach) -> u64 {
        let Some(def) = self.effects.systems.get(&fx).cloned() else {
            return 0;
        };
        self.spawn_def(def, transform, attach)
    }

    pub fn spawn_def(&mut self, def: Arc<SystemDef>, transform: Affine3A, attach: Attach) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let t = if attach == Attach::Camera {
            self.lens_transform()
        } else {
            transform
        };
        let distance = match attach {
            Attach::Viewmodel => t.translation.length(),
            _ => (t.translation - self.camera.translation).length(),
        };
        self.live.push(Live {
            id,
            inst: Instance::new_at_distance(
                def,
                t,
                id.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                distance,
            ),
            attach,
        });
        id
    }

    pub fn set_transform(&mut self, id: u64, transform: Affine3A) {
        if let Some(l) = self.live.iter_mut().find(|l| l.id == id) {
            l.inst.transform = transform;
        }
    }

    pub fn is_live(&self, id: u64) -> bool {
        self.live.iter().any(|l| l.id == id)
    }

    pub fn stop(&mut self, id: u64) {
        if let Some(l) = self.live.iter_mut().find(|l| l.id == id) {
            l.inst.deactivate();
        }
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    fn lens_transform(&self) -> Affine3A {
        let fwd = self.camera.matrix3.x_axis;
        Affine3A {
            matrix3: self.camera.matrix3,
            translation: self.camera.translation + fwd * self.effects.lens_distance,
        }
    }

    fn texture(&mut self, material: &cascade::SpriteMaterial) -> Arc<DishonoredTexture> {
        self.textures
            .entry(material.name.clone())
            .or_insert_with(|| {
                Arc::new(super::hands::texture_with_mips(
                    material.width,
                    material.height,
                    &material.rgba,
                    true,
                ))
            })
            .clone()
    }

    pub fn update(&mut self, dt: f32) -> Vec<DishonoredSprites> {
        let lens = self.lens_transform();
        let eye = self.camera.translation;
        for l in &mut self.live {
            if l.attach == Attach::Camera {
                l.inst.transform = lens;
            }
            let from = if l.attach == Attach::Viewmodel {
                glam::Vec3A::ZERO
            } else {
                eye
            };
            l.inst
                .set_camera_distance((l.inst.transform.translation - from).length());
            l.inst.update(dt);
        }
        self.live.retain(|l| !l.inst.is_finished());

        let basis = self.camera.matrix3;
        let (cam_fwd, cam_right, cam_up) = (
            UVec3::from(basis.x_axis),
            UVec3::from(basis.y_axis),
            UVec3::from(basis.z_axis),
        );
        let cam_eye = UVec3::from(eye);
        let mut batches: HashMap<(bool, String), DishonoredSprites> = HashMap::new();
        let mut materials = Vec::new();
        for li in 0..self.live.len() {
            let attach = self.live[li].attach;
            let (right, up, fwd) = if attach == Attach::Viewmodel {
                (UVec3::X, UVec3::Z, UVec3::NEG_Y)
            } else {
                (cam_right, cam_up, cam_fwd)
            };
            let l = &self.live[li];
            let defs = l.inst.emitter_defs();
            for p in l.inst.particles() {
                let e = &defs[p.emitter];
                if e.is_mesh {
                    continue;
                }
                let Some(mat) = &e.material else { continue };
                let (sx, sy) = (p.size.x.abs() * 0.5, p.size.y.abs() * 0.5);
                let (a, b) = match e.alignment {
                    Some(Alignment::Velocity) => {
                        let v = p.velocity;
                        let d = (v - fwd * v.dot(fwd)).normalize_or(up);
                        let side = fwd.cross(d).normalize_or(right);
                        (side * sx, d * sy)
                    }
                    Some(Alignment::Axis(ax)) => {
                        let n = l
                            .inst
                            .transform
                            .transform_vector3(ax.vec())
                            .normalize_or(UVec3::Z);
                        let t0 = if n.z.abs() < 0.9 {
                            UVec3::Z.cross(n).normalize()
                        } else {
                            UVec3::X.cross(n).normalize()
                        };
                        let t1 = n.cross(t0);
                        let (s, co) = p.rotation.sin_cos();
                        ((t0 * co + t1 * s) * sx, (t1 * co - t0 * s) * sy)
                    }
                    _ => {
                        let (s, co) = p.rotation.sin_cos();
                        ((right * co + up * s) * sx, (up * co - right * s) * sy)
                    }
                };
                let view_space = attach != Attach::World;
                let place = |q: UVec3| -> [f32; 3] {
                    match attach {
                        Attach::World => to_iw(q).to_array(),
                        Attach::Camera => {
                            let d = q - cam_eye;
                            [
                                d.dot(cam_right) * VIEW_SCALE,
                                d.dot(cam_up) * VIEW_SCALE,
                                -d.dot(cam_fwd) * VIEW_SCALE,
                            ]
                        }
                        Attach::Viewmodel => [q.x * VIEW_SCALE, q.z * VIEW_SCALE, q.y * VIEW_SCALE],
                    }
                };
                let col = [
                    p.color[0].max(0.0),
                    p.color[1].max(0.0),
                    p.color[2].max(0.0),
                    p.color[3].clamp(0.0, 1.0),
                ];
                let t = mat.tint;
                let additive = mat.blend == Blend::Additive;
                let (k, alpha) = if additive {
                    (t[3].max(0.05), 1.0)
                } else {
                    (1.0, t[3].clamp(0.0, 1.0))
                };
                let col = [
                    col[0] * t[0] * k,
                    col[1] * t[1] * k,
                    col[2] * t[2] * k,
                    col[3] * alpha,
                ];
                let key = (view_space, mat.name.clone());
                if !batches.contains_key(&key) {
                    materials.push((key.clone(), mat.clone()));
                }
                let batch = batches.entry(key).or_insert_with(|| DishonoredSprites {
                    texture: Arc::new(DishonoredTexture {
                        width: 1,
                        height: 1,
                        srgb: true,
                        levels: vec![vec![255; 4]],
                    }),
                    additive,
                    view_space,
                    vertices: Vec::new(),
                    indices: Vec::new(),
                });
                let base = batch.vertices.len() as u32;
                for (corner, uv) in [
                    (-a - b, [0.0, 1.0]),
                    (a - b, [1.0, 1.0]),
                    (a + b, [1.0, 0.0]),
                    (-a + b, [0.0, 0.0]),
                ] {
                    let [x, y, z] = place(p.pos + corner);
                    batch
                        .vertices
                        .push([x, y, z, uv[0], uv[1], col[0], col[1], col[2], col[3]]);
                }
                batch.indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);
            }
        }
        for (key, mat) in materials {
            let texture = self.texture(&mat);
            if let Some(batch) = batches.get_mut(&key) {
                batch.texture = texture;
            }
        }
        batches.into_values().collect()
    }
}

pub fn placed(pos: UVec3, yaw: f32) -> Affine3A {
    Affine3A::from_mat3_translation(Mat3::from_rotation_z(yaw), pos)
}

pub fn pitched_down(pos: UVec3, yaw: f32) -> Affine3A {
    let (s, c) = yaw.sin_cos();
    let x = UVec3::new(0.0, 0.0, -1.0);
    let y = UVec3::new(-s, c, 0.0);
    let z = UVec3::new(c, s, 0.0);
    Affine3A::from_mat3_translation(Mat3::from_cols(x, y, z), pos)
}

pub fn camera_basis(eye: UVec3, forward: UVec3) -> Affine3A {
    let f = forward.normalize_or_zero();
    let r = UVec3::Z.cross(f).normalize_or(UVec3::Y);
    let u = f.cross(r);
    Affine3A::from_mat3_translation(Mat3::from_cols(f, r, u), eye)
}

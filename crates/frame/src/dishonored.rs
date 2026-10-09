use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct DishonoredMode {
    pub active: bool,
    pub wanted: bool,
    pub toggle_requested: bool,
    pub input_blocked: bool,
    pub ready: bool,
    pub client: u32,
    /// IW4 map space, with Dishonored's vertical FOV in degrees.
    pub camera: Option<(Transform, f32)>,
    pub view_angles: [f32; 3],
    pub status: String,
    /// What the attack button would do now, for the HUD.
    pub prompt: Option<DishonoredPrompt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DishonoredPrompt {
    Assassinate,
    /// A drop assassination, in reach.
    DropAssassinate,
    /// A drop assassination still too far below: attacking locks on and dives.
    DropDive,
}

/// Which clients each living bot has noticed, by bot client id: a bit per client id. The bots
/// write it every authority tick; Dishonored mode reads it as their awareness.
#[derive(Resource, Default)]
pub struct BotNotice(pub std::collections::HashMap<u32, u64>);

pub struct DishonoredTexture {
    pub width: u32,
    pub height: u32,
    pub srgb: bool,
    pub levels: Vec<Vec<u8>>,
}

pub struct DishonoredMesh {
    pub texture: Option<std::sync::Arc<DishonoredTexture>>,
    /// View space in metres, -Z forward: position, normal, uv.
    pub vertices: Vec<[f32; 8]>,
    pub indices: std::sync::Arc<Vec<u32>>,
}

pub struct DishonoredSprites {
    pub texture: std::sync::Arc<DishonoredTexture>,
    pub additive: bool,
    /// View space in metres like the arms, else IW4 map inches.
    pub view_space: bool,
    pub vertices: Vec<[f32; 9]>,
    pub indices: Vec<u32>,
}

#[derive(Resource, Default)]
pub struct DishonoredDraw {
    pub active: bool,
    pub fov_y: f32,
    pub near: f32,
    pub light_dir: [f32; 3],
    pub meshes: Vec<DishonoredMesh>,
    pub sprites: Vec<DishonoredSprites>,
    pub vignette: f32,
}

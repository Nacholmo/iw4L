use std::collections::HashMap;
use std::sync::Arc;

use bevy::core_pipeline::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntry, BindGroupLayoutDescriptor, BindingResource,
    BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, BufferBinding,
    BufferDescriptor, BufferInitDescriptor, BufferUsages, ColorTargetState, ColorWrites,
    CompareFunction, DepthStencilState, Extent3d, FilterMode, IndexFormat, MipmapFilterMode,
    MultisampleState, Origin3d, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, RawFragmentState, RawRenderPipelineDescriptor, RawVertexBufferLayout,
    RawVertexState, RenderPipeline, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, StoreOp, TexelCopyBufferLayout,
    TexelCopyTextureInfo, TextureAspect, TextureDescriptor, TextureDimension, TextureFormat,
    TextureSampleType, TextureUsages, TextureViewDescriptor, VertexAttribute, VertexFormat,
    VertexStepMode,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery};
use bevy::render::view::{ExtractedView, Msaa, ViewTarget};
use bevy::render::{ExtractSchedule, MainWorld, Render, RenderApp, RenderSystems};
use frame::{DishonoredDraw, DishonoredTexture};

use super::depth_range::{
    GFX_DEPTH_RANGE_SCENE, GFX_DEPTH_RANGE_VIEWMODEL, reverse_z_viewport_depth,
};
use super::exact_pipeline::ExactPipelineRegistry;
use super::scene_depth::{SCENE_DEPTH_FORMAT, SceneDepthTexture};

#[derive(Resource, Default)]
pub struct DishonoredFrame(pub DishonoredDraw);

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct View {
    clip_from_world: [f32; 16],
    viewmodel_clip: [f32; 16],
    light_dir: [f32; 4],
    params: [f32; 4],
}

const VIEW_SIZE: u64 = std::mem::size_of::<View>() as u64;
const MESH_VERTEX_BYTES: u64 = 32;
const SPRITE_VERTEX_BYTES: u64 = 36;

struct Draw {
    texture: usize,
    vertices: Buffer,
    indices: Buffer,
    count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pass {
    Mesh,
    WorldAdd,
    WorldAlpha,
    ViewAdd,
    ViewAlpha,
    Vignette,
}

#[derive(Resource, Default)]
struct DishonoredGpu {
    active: bool,
    vignette: bool,
    view: Option<Buffer>,
    view_bind: Option<BindGroup>,
    sampler: Option<Sampler>,
    white: Option<Arc<DishonoredTexture>>,
    textures: HashMap<usize, (Arc<DishonoredTexture>, BindGroup)>,
    draws: Vec<(Pass, Draw)>,
    pipelines: HashMap<(TextureFormat, u32), HashMap<Pass, RenderPipeline>>,
}

pub(super) fn register(app: &mut App) {
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app
        .init_resource::<DishonoredFrame>()
        .init_resource::<DishonoredGpu>()
        .add_systems(ExtractSchedule, extract)
        .add_systems(Render, prepare.in_set(RenderSystems::PrepareResources))
        .add_systems(
            Core3d,
            draw.in_set(Core3dSystems::MainPass)
                .after(super::draw::ExactColourDrawSet),
        );
}

fn extract(mut main: ResMut<MainWorld>, mut frame: ResMut<DishonoredFrame>) {
    let Some(mut draw) = main.get_resource_mut::<DishonoredDraw>() else {
        frame.0.active = false;
        return;
    };
    frame.0.active = draw.active;
    frame.0.fov_y = draw.fov_y;
    frame.0.near = draw.near;
    frame.0.light_dir = draw.light_dir;
    frame.0.vignette = draw.vignette;
    frame.0.meshes = std::mem::take(&mut draw.meshes);
    frame.0.sprites = std::mem::take(&mut draw.sprites);
}

fn texture_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "iw4l_dishonored_texture",
        &[
            texture_2d(TextureSampleType::Float { filterable: true })
                .visibility(ShaderStages::FRAGMENT)
                .build(0, ShaderStages::FRAGMENT),
            sampler(SamplerBindingType::Filtering)
                .visibility(ShaderStages::FRAGMENT)
                .build(1, ShaderStages::FRAGMENT),
        ],
    )
}

fn view_layout() -> BindGroupLayoutDescriptor {
    BindGroupLayoutDescriptor::new(
        "iw4l_dishonored_view",
        &[
            uniform_buffer_sized(false, std::num::NonZeroU64::new(VIEW_SIZE))
                .visibility(ShaderStages::VERTEX_FRAGMENT)
                .build(0, ShaderStages::VERTEX_FRAGMENT),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    mut frame: ResMut<DishonoredFrame>,
    published: Option<Res<super::PublishedRenderFrame>>,
    views: Query<&ExtractedView>,
    registry: Res<ExactPipelineRegistry>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut gpu: ResMut<DishonoredGpu>,
) {
    let gpu = &mut *gpu;
    gpu.draws.clear();
    gpu.active = frame.0.active;
    if !gpu.active {
        return;
    }
    if gpu.view.is_none() {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("iw4l_dishonored_view"),
            size: VIEW_SIZE,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = registry.bind_group_layout(&device, &view_layout());
        gpu.view_bind = Some(device.create_bind_group(
            "iw4l_dishonored_view",
            &layout,
            &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: None,
                }),
            }],
        ));
        gpu.view = Some(buffer);
        gpu.sampler = Some(device.create_sampler(&SamplerDescriptor {
            label: Some("iw4l_dishonored"),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Linear,
            ..default()
        }));
        gpu.white = Some(Arc::new(DishonoredTexture {
            width: 1,
            height: 1,
            srgb: true,
            levels: vec![vec![255; 4]],
        }));
    }
    let white = gpu.white.clone().expect("white texture");
    let draw = &mut frame.0;
    let texture_key = |texture: &Arc<DishonoredTexture>, gpu: &mut DishonoredGpu| -> usize {
        let key = Arc::as_ptr(texture) as usize;
        if !gpu.textures.contains_key(&key) {
            let view = upload(&device, &queue, texture);
            let layout = registry.bind_group_layout(&device, &texture_layout());
            let bind = device.create_bind_group(
                "iw4l_dishonored_texture",
                &layout,
                &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(gpu.sampler.as_ref().expect("sampler")),
                    },
                ],
            );
            gpu.textures.insert(key, (texture.clone(), bind));
        }
        key
    };
    texture_key(&white, gpu);
    for mesh in std::mem::take(&mut draw.meshes) {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            continue;
        }
        let texture = texture_key(mesh.texture.as_ref().unwrap_or(&white), gpu);
        gpu.draws.push((
            Pass::Mesh,
            Draw {
                texture,
                vertices: device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("iw4l_dishonored_mesh_vertices"),
                    contents: bytemuck::cast_slice(&mesh.vertices),
                    usage: BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("iw4l_dishonored_mesh_indices"),
                    contents: bytemuck::cast_slice(&mesh.indices),
                    usage: BufferUsages::INDEX,
                }),
                count: mesh.indices.len() as u32,
            },
        ));
    }
    let mut sprites = std::mem::take(&mut draw.sprites);
    sprites.sort_by_key(|s| (s.view_space, s.additive));
    for sprite in sprites {
        if sprite.indices.is_empty() {
            continue;
        }
        let texture = texture_key(&sprite.texture, gpu);
        let pass = match (sprite.view_space, sprite.additive) {
            (false, true) => Pass::WorldAdd,
            (false, false) => Pass::WorldAlpha,
            (true, true) => Pass::ViewAdd,
            (true, false) => Pass::ViewAlpha,
        };
        gpu.draws.push((
            pass,
            Draw {
                texture,
                vertices: device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("iw4l_dishonored_sprite_vertices"),
                    contents: bytemuck::cast_slice(&sprite.vertices),
                    usage: BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some("iw4l_dishonored_sprite_indices"),
                    contents: bytemuck::cast_slice(&sprite.indices),
                    usage: BufferUsages::INDEX,
                }),
                count: sprite.indices.len() as u32,
            },
        ));
    }
    gpu.vignette = draw.vignette > 0.002;

    let aspect = views
        .iter()
        .next()
        .map(|v| v.viewport.z as f32 / (v.viewport.w as f32).max(1.0))
        .unwrap_or(16.0 / 9.0);
    let mut view = View {
        viewmodel_clip: Mat4::perspective_infinite_reverse_rh(
            draw.fov_y.max(0.1),
            aspect,
            draw.near.max(0.001),
        )
        .to_cols_array(),
        light_dir: [draw.light_dir[0], draw.light_dir[1], draw.light_dir[2], 0.0],
        params: [draw.vignette, 0.42, 0.75, 0.0],
        ..View::default()
    };
    if let Some(clip_from_world) = published
        .as_ref()
        .and_then(|p| p.exec_frame.clip_from_world)
    {
        view.clip_from_world = clip_from_world.to_cols_array();
    }
    if let Some(buffer) = gpu.view.as_ref() {
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&view));
    }
}

fn upload(
    device: &RenderDevice,
    queue: &RenderQueue,
    image: &DishonoredTexture,
) -> bevy::render::render_resource::TextureView {
    let levels = image.levels.len().max(1) as u32;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("iw4l_dishonored_texture"),
        size: Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: TextureDimension::D2,
        // The colour target holds display values, as IW4's own textures do.
        format: TextureFormat::Rgba8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, texels) in image.levels.iter().enumerate() {
        let (w, h) = (
            (image.width >> level).max(1),
            (image.height >> level).max(1),
        );
        if texels.len() < (w * h * 4) as usize {
            break;
        }
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            texels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&TextureViewDescriptor::default())
}

fn draw(
    view: ViewQuery<(
        &ViewTarget,
        &SceneDepthTexture,
        &ExtractedView,
        Option<&Msaa>,
    )>,
    registry: Res<ExactPipelineRegistry>,
    device: Res<RenderDevice>,
    mut gpu: ResMut<DishonoredGpu>,
    mut context: RenderContext,
) {
    if !gpu.active || (gpu.draws.is_empty() && !gpu.vignette) {
        return;
    }
    let Some(view_bind) = gpu.view_bind.clone() else {
        return;
    };
    let (target, depth, extracted_view, msaa) = view.into_inner();
    let format = target.main_texture_format();
    let samples = msaa.map_or(1, Msaa::samples);
    let pipelines = gpu
        .pipelines
        .entry((format, samples))
        .or_insert_with(|| pipelines(&device, &registry, format, samples))
        .clone();
    let attachments = [Some(target.get_color_attachment())];
    let mut pass =
        context.begin_tracked_render_pass(bevy::render::render_resource::RenderPassDescriptor {
            label: Some("iw4l_dishonored"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    let vp = extracted_view.viewport;
    let band = |range| {
        let (lo, hi) = reverse_z_viewport_depth(range);
        (vp.x as f32, vp.y as f32, vp.z as f32, vp.w as f32, lo, hi)
    };
    pass.set_bind_group(0, &view_bind, &[]);
    for (kind, d) in &gpu.draws {
        let Some((_, bind)) = gpu.textures.get(&d.texture) else {
            continue;
        };
        let (x, y, w, h, lo, hi) = band(match kind {
            Pass::WorldAdd | Pass::WorldAlpha => GFX_DEPTH_RANGE_SCENE,
            _ => GFX_DEPTH_RANGE_VIEWMODEL,
        });
        pass.set_viewport(x, y, w, h, lo, hi);
        pass.set_render_pipeline(&pipelines[kind]);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, d.vertices.slice(..));
        pass.set_index_buffer(d.indices.slice(..), IndexFormat::Uint32);
        pass.draw_indexed(0..d.count, 0, 0..1);
    }
    if gpu.vignette
        && let Some((_, bind)) = gpu
            .white
            .as_ref()
            .and_then(|w| gpu.textures.get(&(Arc::as_ptr(w) as usize)))
    {
        let (x, y, w, h, lo, hi) = band(GFX_DEPTH_RANGE_VIEWMODEL);
        pass.set_viewport(x, y, w, h, lo, hi);
        pass.set_render_pipeline(&pipelines[&Pass::Vignette]);
        pass.set_bind_group(1, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

const WGSL: &str = r#"
struct View {
    clip_from_world: mat4x4<f32>,
    viewmodel_clip: mat4x4<f32>,
    light_dir: vec4<f32>,
    params: vec4<f32>,
}
@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn mesh_vertex(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> MeshOut {
    var out: MeshOut;
    out.clip = view.viewmodel_clip * vec4<f32>(position, 1.0);
    out.normal = normal;
    out.uv = uv;
    return out;
}

@fragment
fn mesh_fragment(in: MeshOut) -> @location(0) vec4<f32> {
    let texel = textureSample(image, image_sampler, in.uv);
    let n = normalize(in.normal);
    let key = max(abs(dot(n, view.light_dir.xyz)) * select(0.35, 1.0, dot(n, view.light_dir.xyz) > 0.0), 0.0);
    let rim = pow(1.0 - abs(n.z), 3.0) * 0.12;
    let lit = texel.rgb * (view.params.y + view.params.z * key + rim);
    return vec4<f32>(lit, 1.0);
}

struct SpriteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
}

@vertex
fn world_sprite_vertex(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
) -> SpriteOut {
    var out: SpriteOut;
    out.clip = view.clip_from_world * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.colour = colour;
    return out;
}

@vertex
fn view_sprite_vertex(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
) -> SpriteOut {
    var out: SpriteOut;
    out.clip = view.viewmodel_clip * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.colour = colour;
    return out;
}

@fragment
fn sprite_fragment(in: SpriteOut) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, in.uv) * in.colour;
}

struct VignetteOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vignette_vertex(@builtin(vertex_index) index: u32) -> VignetteOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VignetteOut;
    out.ndc = uv * 2.0 - 1.0;
    out.clip = vec4<f32>(out.ndc, 0.0, 1.0);
    return out;
}

@fragment
fn vignette_fragment(in: VignetteOut) -> @location(0) vec4<f32> {
    let d = sqrt(in.ndc.x * in.ndc.x * 0.8 + in.ndc.y * in.ndc.y);
    let a = pow(clamp((d - 0.45) / 0.75, 0.0, 1.0), 1.6) * view.params.x;
    return vec4<f32>(0.0, 0.0, 0.0, a);
}
"#;

fn pipelines(
    device: &RenderDevice,
    registry: &ExactPipelineRegistry,
    format: TextureFormat,
    samples: u32,
) -> HashMap<Pass, RenderPipeline> {
    let shader = unsafe {
        device.create_shader_module(ShaderModuleDescriptor {
            label: Some("iw4l_dishonored"),
            source: ShaderSource::Wgsl(WGSL.into()),
        })
    };
    let view = registry.bind_group_layout(device, &view_layout());
    let texture = registry.bind_group_layout(device, &texture_layout());
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("iw4l_dishonored"),
        bind_group_layouts: &[Some(&view), Some(&texture)],
        immediate_size: 0,
    });
    let mesh_attributes = [
        VertexAttribute {
            format: VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        VertexAttribute {
            format: VertexFormat::Float32x3,
            offset: 12,
            shader_location: 1,
        },
        VertexAttribute {
            format: VertexFormat::Float32x2,
            offset: 24,
            shader_location: 2,
        },
    ];
    let mesh_buffers = [RawVertexBufferLayout {
        array_stride: MESH_VERTEX_BYTES,
        step_mode: VertexStepMode::Vertex,
        attributes: &mesh_attributes,
    }];
    let sprite_attributes = [
        VertexAttribute {
            format: VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        VertexAttribute {
            format: VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        VertexAttribute {
            format: VertexFormat::Float32x4,
            offset: 20,
            shader_location: 2,
        },
    ];
    let sprite_buffers = [RawVertexBufferLayout {
        array_stride: SPRITE_VERTEX_BYTES,
        step_mode: VertexStepMode::Vertex,
        attributes: &sprite_attributes,
    }];
    let additive = BlendState {
        color: BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        },
        alpha: BlendComponent {
            src_factor: BlendFactor::Zero,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        },
    };
    let options = PipelineCompilationOptions {
        constants: &[],
        zero_initialize_workgroup_memory: false,
    };
    let make = |vertex: &str,
                fragment: &str,
                buffers: &[RawVertexBufferLayout],
                depth_write: bool,
                compare: CompareFunction,
                blend: Option<BlendState>| {
        device.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some("iw4l_dishonored"),
            layout: Some(&layout),
            vertex: RawVertexState {
                module: &shader,
                entry_point: Some(vertex),
                buffers,
                compilation_options: options.clone(),
            },
            fragment: Some(RawFragmentState {
                module: &shader,
                entry_point: Some(fragment),
                targets: &[Some(ColorTargetState {
                    format,
                    blend,
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: options.clone(),
            }),
            // Unreal to view space mirrors handedness, so the arms wind backwards.
            primitive: PrimitiveState {
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: SCENE_DEPTH_FORMAT,
                depth_write_enabled: Some(depth_write),
                depth_compare: Some(compare),
                stencil: default(),
                bias: default(),
            }),
            multisample: MultisampleState {
                count: samples,
                ..default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let ge = CompareFunction::GreaterEqual;
    let alpha = Some(BlendState::ALPHA_BLENDING);
    HashMap::from([
        (
            Pass::Mesh,
            make(
                "mesh_vertex",
                "mesh_fragment",
                &mesh_buffers,
                true,
                ge,
                None,
            ),
        ),
        (
            Pass::WorldAdd,
            make(
                "world_sprite_vertex",
                "sprite_fragment",
                &sprite_buffers,
                false,
                ge,
                Some(additive),
            ),
        ),
        (
            Pass::WorldAlpha,
            make(
                "world_sprite_vertex",
                "sprite_fragment",
                &sprite_buffers,
                false,
                ge,
                alpha,
            ),
        ),
        (
            Pass::ViewAdd,
            make(
                "view_sprite_vertex",
                "sprite_fragment",
                &sprite_buffers,
                false,
                ge,
                Some(additive),
            ),
        ),
        (
            Pass::ViewAlpha,
            make(
                "view_sprite_vertex",
                "sprite_fragment",
                &sprite_buffers,
                false,
                ge,
                alpha,
            ),
        ),
        (
            Pass::Vignette,
            make(
                "vignette_vertex",
                "vignette_fragment",
                &[],
                false,
                CompareFunction::Always,
                alpha,
            ),
        ),
    ])
}

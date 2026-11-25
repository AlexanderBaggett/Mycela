//! Offscreen portable scene rasterization. Scene/mesh/camera owners remain in
//! mycela-engine; this crate owns only device resources and render snapshots.
mod admission;
mod mesh;
use mycela_engine::{
    CameraError, ImageError, ImageGeometry, ImageMap, Scene, SceneCamera, SceneError,
    SceneMaterial, SceneTransform, SharedImageMap, UvLightSettings, UvLightShadow,
};
use std::{
    borrow::Cow,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;

#[derive(Debug)]
pub enum SceneRenderError<E> {
    Camera(CameraError<E>),
    Scene(SceneError),
    Image(ImageError),
    MissingCameraMatrix,
    InvalidGeometry,
    InvalidMesh,
    Allocation,
    MemoryQuery(std::io::Error),
    InsufficientMemory,
    UnsupportedFeature(&'static str),
    Device(String),
    Cancelled,
    TimedOut,
}
impl<E: fmt::Display> fmt::Display for SceneRenderError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Camera(e) => write!(f, "{e}"),
            Self::Scene(e) => write!(f, "{e}"),
            Self::Image(e) => write!(f, "{e}"),
            Self::MemoryQuery(e) => write!(f, "Scene memory query failed: {e}"),
            Self::Device(e) => write!(f, "Scene device error: {e}"),
            Self::MissingCameraMatrix => f.write_str("Camera returned no matrix"),
            Self::InvalidGeometry => {
                f.write_str("Scene dimensions exceed the device or integer layout")
            }
            Self::InvalidMesh => f.write_str("Scene corner channels are invalid"),
            Self::Allocation => f.write_str("Scene buffer allocation failed"),
            Self::InsufficientMemory => {
                f.write_str("Insufficient available memory for the scene frame")
            }
            Self::UnsupportedFeature(name) => {
                write!(f, "Scene renderer capability is unfinished: {name}")
            }
            Self::Cancelled => f.write_str("Scene render cancelled"),
            Self::TimedOut => f.write_str("Scene render timed out"),
        }
    }
}
impl<E: std::error::Error + 'static> std::error::Error for SceneRenderError<E> {}
/// Explicit caller deadline and cancellation, with the source node's four
/// depth-peeling layers supplied by source_node(). No UI state is retained.
pub struct SceneRenderRequest<'a> {
    width: u32,
    height: u32,
    layers: i32,
    cancelled: &'a AtomicBool,
    cancel_check: Option<&'a dyn Fn() -> bool>,
    deadline: Instant,
    uv_light: Option<(UvLightSettings, Option<&'a UvLightShadow>)>,
    uv_material_camera: bool,
}
impl<'a> SceneRenderRequest<'a> {
    pub fn source_node(
        width: u32,
        height: u32,
        cancelled: &'a AtomicBool,
        deadline: Instant,
    ) -> Self {
        Self {
            width,
            height,
            layers: 4,
            cancelled,
            cancel_check: None,
            deadline,
            uv_light: None,
            uv_material_camera: false,
        }
    }
    pub fn with_layers(mut self, layers: i32) -> Self {
        self.layers = layers;
        self
    }
    pub fn with_uv_light(
        mut self,
        light: UvLightSettings,
        shadow: Option<&'a UvLightShadow>,
    ) -> Self {
        self.uv_light = Some((light, shadow));
        self
    }
    /// The source's UV Curvature and UV Thickness camera output uses a single
    /// full-precision layer and reads the OpenGL rows in atlas order.
    pub fn with_uv_vertex_map(self) -> Self {
        self.with_layers(1)
    }
    /// Keep the UV Environment Light material camera's existing layer quantization.
    pub fn with_uv_material_camera(mut self) -> Self {
        self.uv_material_camera = true;
        self
    }
    /// Observe the caller's actual generation/closed-state predicate at the same
    /// checkpoints as the atomic token. The predicate is borrowed for this render
    /// only; no model guard spans it and no polling thread or mirrored flag exists.
    pub fn with_cancel_check(mut self, cancel_check: &'a dyn Fn() -> bool) -> Self {
        self.cancel_check = Some(cancel_check);
        self
    }

    fn check<E>(&self) -> Result<(), SceneRenderError<E>> {
        if self.cancelled.load(Ordering::Acquire) || self.cancel_check.is_some_and(|check| check())
        {
            Err(SceneRenderError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(SceneRenderError::TimedOut)
        } else {
            Ok(())
        }
    }
}
/// Real device-readback pixels in the request's image row order. Alpha is opacity;
/// the existing engine's fourth map is transparency and is converted on import.
pub struct SceneFrame {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 4]>,
}
impl SceneFrame {
    pub const fn width(&self) -> u32 {
        self.width
    }
    pub const fn height(&self) -> u32 {
        self.height
    }
    pub fn pixels(&self) -> &[[f32; 4]] {
        &self.pixels
    }
    pub fn into_image(self) -> Result<SharedImageMap, ImageError> {
        let mut channels: [Vec<f32>; 4] = std::array::from_fn(|_| Vec::new());
        for channel in &mut channels {
            channel
                .try_reserve_exact(self.pixels.len())
                .map_err(|_| ImageError::Allocation)?;
        }
        for pixel in self.pixels {
            for (ix, value) in pixel.into_iter().enumerate() {
                channels[ix].push(if ix == 3 { 1. - value } else { value });
            }
        }
        let mut image = ImageMap::new(ImageGeometry::new(self.width as i32, self.height as i32));
        for (ix, values) in channels.into_iter().enumerate() {
            image.set_array(ix as i32, values)?;
        }
        image.seal_permanently()?;
        Ok(SharedImageMap::from_image(image))
    }
}
/// Actual GL_RGBA layer precision is implementation-selected in the source.
/// Supply the observed source profile; no float-transfer argument is treated as
/// proof of floating-point storage. The final source qs target requests RGBA16.
#[derive(Clone, Copy, Debug)]
pub enum SceneLayerColor {
    Normalized8,
    Normalized16,
    /// The Genetica OpenGL profile measured under Wine. Standard camera layers
    /// truncate RGB to binary16 precision before rounding to 8-bit UNORM.
    GeneticaOpenGl8,
}
#[derive(Clone, Copy, Debug)]
pub enum SceneDepthPrecision {
    AtLeast24,
    Float32,
}
#[derive(Clone, Copy, Debug)]
pub struct SceneTargetProfile {
    layers: SceneLayerColor,
    depth: SceneDepthPrecision,
}
impl SceneTargetProfile {
    pub const fn new(layers: SceneLayerColor, depth: SceneDepthPrecision) -> Self {
        Self { layers, depth }
    }
    fn layer_format(self) -> wgpu::TextureFormat {
        match self.layers {
            SceneLayerColor::Normalized8 | SceneLayerColor::GeneticaOpenGl8 => {
                wgpu::TextureFormat::Rgba8Unorm
            }
            SceneLayerColor::Normalized16 => wgpu::TextureFormat::Rgba16Unorm,
        }
    }
    fn layer_bytes(self) -> u32 {
        match self.layers {
            SceneLayerColor::Normalized8 | SceneLayerColor::GeneticaOpenGl8 => 4,
            SceneLayerColor::Normalized16 => 8,
        }
    }
    fn depth_format(self) -> wgpu::TextureFormat {
        match self.depth {
            SceneDepthPrecision::AtLeast24 => wgpu::TextureFormat::Depth24Plus,
            SceneDepthPrecision::Float32 => wgpu::TextureFormat::Depth32Float,
        }
    }
}
/// Snapshots are taken at each source uniform upload, before later factories
/// can mutate a retained matrix. No camera/scene guard spans a callback.
struct CameraUpload {
    view: SceneTransform,
    normal: [f64; 9],
    composed: SceneTransform,
    light_view: SceneTransform,
}
fn read_camera<E: 'static>(camera: &SceneCamera<E>) -> Result<CameraUpload, SceneRenderError<E>> {
    // ba.ah: v, f, s, u, n, v, mp.X, mp.Y, mp.Z. The ordinary shader
    // does not consume inverse/projection/position, but their effects/errors exist.
    let view = camera
        .view()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .value();
    let _inverse = camera
        .inverse_view()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .value()
        .values()
        .map(|v| v as f32);
    let normal = camera
        .normal()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .values();
    let _projection = camera
        .projection()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .value()
        .values()
        .map(|v| v as f32);
    let composed = camera
        .view_projection()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .value();
    let light_view = camera
        .view()
        .map_err(SceneRenderError::Camera)?
        .ok_or(SceneRenderError::MissingCameraMatrix)?
        .value();
    let _position = [
        0.0_f32 - camera.position().values()[0] as f32,
        camera.position().values()[1] as f32,
        camera.position().values()[2] as f32,
        1.,
    ];
    Ok(CameraUpload {
        view,
        normal,
        composed,
        light_view,
    })
}
/// ama branches on >1 before allocating peel resources. Zero and negative
/// layer requests take the genuine one-pass final framebuffer route as well.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RenderRoute {
    Direct,
    Peel(u32),
}
impl RenderRoute {
    fn new(layers: i32) -> Self {
        if layers > 1 {
            Self::Peel(layers as u32)
        } else {
            Self::Direct
        }
    }
}
fn composite_projection(width: u32, height: u32) -> [f32; 16] {
    // ama.r7 -> nd.a -> ab7 rect setter -> awx.a -> apt.a. Keep the
    // original Double operations, including reconstructed right/top and signs.
    let aspect = f64::from(width) / f64::from(height);
    let left = -aspect;
    let bottom = -1.;
    let rectangle_width = aspect - left;
    let rectangle_height = 1. - bottom;
    let right = left + rectangle_width;
    let top = bottom + rectangle_height;
    [
        4. / (right - left),
        0.,
        0.,
        0.,
        0.,
        4. / (top - bottom),
        0.,
        0.,
        0.,
        0.,
        0.,
        0.,
        -(right + left + 2.) / (right - left),
        -(bottom + top - 2.) / (bottom - top),
        0.,
        1.,
    ]
    .map(|v| v as f32)
}
fn composite_sampler() -> wgpu::SamplerDescriptor<'static> {
    // New GL textures retain REPEAT; f.a sets both min/mag to GL_LINEAR.
    wgpu::SamplerDescriptor {
        label: Some("Source linear/repeat color sampling"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: 0.,
        ..Default::default()
    }
}
struct GpuMesh {
    vertex: wgpu::Buffer,
    count: u32,
}
type MaterialTextureUpload = (wgpu::Texture, Option<wgpu::Buffer>, u32, bool, u64);

struct MaterialGroup {
    material: SceneMaterial,
    texture: wgpu::Texture,
    staging: Option<wgpu::Buffer>,
    row_pitch: u32,
    textured: bool,
    meshes: Vec<GpuMesh>,
}
/// One offscreen GPU device. render() belongs on the existing background worker,
/// never inside a GPUI render callback. It returns actual readback or a failure.
pub struct SceneRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    lighting_peel: wgpu::RenderPipeline,
    lighting_direct: wgpu::RenderPipeline,
    lighting_bindings: wgpu::BindGroupLayout,
    composite: wgpu::RenderPipeline,
    composite_bindings: wgpu::BindGroupLayout,
    linear_repeat: wgpu::Sampler,
    errors: Arc<Mutex<Vec<String>>>,
    adapter: wgpu::AdapterInfo,
    profile: SceneTargetProfile,
}
impl SceneRenderer {
    pub async fn new(
        profile: SceneTargetProfile,
    ) -> Result<Self, SceneRenderError<std::convert::Infallible>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|e| SceneRenderError::Device(e.to_string()))?;
        let needed = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        for format in [profile.layer_format(), wgpu::TextureFormat::Rgba16Unorm] {
            if !adapter
                .get_texture_format_features(format)
                .allowed_usages
                .contains(needed)
            {
                return Err(SceneRenderError::UnsupportedFeature(
                    "source normalized color attachment",
                ));
            }
        }
        let map_features = adapter.get_texture_format_features(wgpu::TextureFormat::Rgba16Unorm);
        if !map_features
            .allowed_usages
            .contains(wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST)
            || !map_features
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
        {
            return Err(SceneRenderError::UnsupportedFeature(
                "linear normalized material texture",
            ));
        }
        if !adapter
            .get_texture_format_features(profile.layer_format())
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
        {
            return Err(SceneRenderError::UnsupportedFeature(
                "source linear normalized layer sampling",
            ));
        }
        if !adapter
            .features()
            .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
        {
            return Err(SceneRenderError::UnsupportedFeature(
                "source RGBA16 normalized target",
            ));
        }
        let features = (adapter.features()
            & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Mycela scene renderer"),
                required_features: features,
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| SceneRenderError::Device(e.to_string()))?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let captured = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(error.to_string())
        }));
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let lighting_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Original standard lighting translation"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("standard_lighting.wgsl"))),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Original layer composite translation"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("composite.wgsl"))),
        });
        let lighting_bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shared direct/peel lighting resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(256),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(112),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let lighting_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Scene lighting layout"),
            bind_group_layouts: &[Some(&lighting_bindings)],
            immediate_size: 0,
        });
        let lighting_peel = lighting_pipeline(
            &device,
            &lighting_shader,
            &lighting_layout,
            profile.layer_format(),
            profile.depth_format(),
            "Scene lighting and depth peeling",
        );
        // qs requests GL_DEPTH_COMPONENT32. Depth32Float is the explicit portable
        // mapping here; original normalized-vs-float precision still needs capture.
        let lighting_direct = lighting_pipeline(
            &device,
            &lighting_shader,
            &lighting_layout,
            wgpu::TextureFormat::Rgba16Unorm,
            wgpu::TextureFormat::Depth32Float,
            "Scene direct final RGBA16 lighting",
        );
        let composite_bindings =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Source projected linear layer reads"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Scene composite layout"),
            bind_group_layouts: &[Some(&composite_bindings)],
            immediate_size: 0,
        });
        let final_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba16Unorm,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Source unit-UV straight-alpha composition"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &final_targets,
            }),
            multiview_mask: None,
            cache: None,
        });
        let linear_repeat = device.create_sampler(&composite_sampler());
        if let Some(error) = validation.pop().await {
            return Err(SceneRenderError::Device(error.to_string()));
        }
        let renderer = Self {
            device,
            queue,
            lighting_peel,
            lighting_direct,
            lighting_bindings,
            composite,
            composite_bindings,
            linear_repeat,
            errors,
            adapter: adapter.get_info(),
            profile,
        };
        renderer.check_device()?;
        Ok(renderer)
    }
    pub fn adapter(&self) -> &wgpu::AdapterInfo {
        &self.adapter
    }
    fn check_device<E>(&self) -> Result<(), SceneRenderError<E>> {
        let mut errors = self.errors.lock().unwrap_or_else(|e| e.into_inner());
        if errors.is_empty() {
            Ok(())
        } else {
            Err(SceneRenderError::Device(
                std::mem::take(&mut *errors).join("\n"),
            ))
        }
    }
    fn texture(
        &self,
        width: u32,
        height: u32,
        layers: u32,
        format: wgpu::TextureFormat,
        depth: bool,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(if depth { "Scene depth" } else { "Scene color" }),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if depth {
                    wgpu::TextureUsages::empty()
                } else {
                    wgpu::TextureUsages::COPY_SRC
                },
            view_formats: &[],
        })
    }
    // Single-part ColorMap, as uploaded by f.a and sampled by standard lighting.
    // The source uses unsized GL_RGBA; 16-bit normalized is an explicit portable
    // choice and its precision is not claimed to match every GL implementation.
    /// Active selected WebGPU device limit, used by the material assignment path.
    pub fn max_material_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }
    fn material_texture<E>(
        &self,
        map: Option<&SharedImageMap>,
        request: &SceneRenderRequest<'_>,
        reservation: &admission::Reservation,
    ) -> Result<MaterialTextureUpload, SceneRenderError<E>> {
        let (width, height) = match map {
            Some(map) => {
                let geometry = map.geometry();
                let width = u32::try_from(geometry.width())
                    .map_err(|_| SceneRenderError::InvalidGeometry)?;
                let height = u32::try_from(geometry.height())
                    .map_err(|_| SceneRenderError::InvalidGeometry)?;
                if width == 0
                    || height == 0
                    || width > self.device.limits().max_texture_dimension_2d
                    || height > self.device.limits().max_texture_dimension_2d
                {
                    return Err(SceneRenderError::UnsupportedFeature(
                        "material image resizing or invalid dimensions",
                    ));
                }
                (width, height)
            }
            None => (1, 1), // Binding required by WebGPU; never sampled without a map.
        };
        let count = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let bytes = count
            .checked_mul(8)
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let row_pitch = width
            .checked_mul(8)
            .and_then(|n| n.checked_add(255))
            .map(|n| n & !255)
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let staging_bytes = if map.is_some() {
            u64::from(row_pitch) * u64::from(height)
        } else {
            0
        };
        let gpu_bytes = bytes
            .checked_add(staging_bytes)
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let working = gpu_bytes
            .checked_add(staging_bytes)
            .ok_or(SceneRenderError::InvalidGeometry)?;
        if staging_bytes > self.device.limits().max_buffer_size {
            return Err(SceneRenderError::InvalidGeometry);
        }
        reservation.add(working)?;
        let data = if let Some(map) = map {
            let size =
                usize::try_from(staging_bytes).map_err(|_| SceneRenderError::InvalidGeometry)?;
            let mut data = Vec::new();
            data.try_reserve_exact(size)
                .map_err(|_| SceneRenderError::Allocation)?;
            data.resize(size, 0);
            for index in 0..4 {
                request.check()?;
                let buffer = map.channel(index, false).map_err(SceneRenderError::Image)?;
                let plane = buffer.len() > 1;
                for pixel in
                    0..usize::try_from(count).map_err(|_| SceneRenderError::InvalidGeometry)?
                {
                    if pixel % 1024 == 0 {
                        request.check()?;
                    }
                    let mut value = buffer
                        .read(if plane { pixel } else { 0 })
                        .map_err(SceneRenderError::Image)?;
                    if index == 3 {
                        value = 1.0 - value;
                    }
                    if !value.is_finite() {
                        return Err(SceneRenderError::UnsupportedFeature(
                            "nonfinite material image values",
                        ));
                    }
                    let value = (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
                    let offset = (pixel / width as usize) * row_pitch as usize
                        + (pixel % width as usize) * 8
                        + (index as usize) * 2;
                    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
                }
                map.seal_channel(index).map_err(SceneRenderError::Image)?;
            }
            map.seal_permanently().map_err(SceneRenderError::Image)?;
            Some(data)
        } else {
            None
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Actual material ColorMap"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let staging = data.as_ref().map(|data| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Material image upload"),
                    contents: data,
                    usage: wgpu::BufferUsages::COPY_SRC,
                })
        });
        Ok((texture, staging, row_pitch, map.is_some(), gpu_bytes))
    }
    fn upload_meshes<E>(
        &self,
        scene: &Scene,
        request: &SceneRenderRequest<'_>,
        reservation: &admission::Reservation,
    ) -> Result<Vec<MaterialGroup>, SceneRenderError<E>> {
        let models = scene.models();
        let shadow_map = request
            .uv_light
            .and_then(|(_, shadow)| shadow)
            .map(UvLightShadow::image)
            .transpose()
            .map_err(SceneRenderError::Image)?;
        let mut groups: Vec<MaterialGroup> = Vec::new();
        groups
            .try_reserve_exact(models.len())
            .map_err(|_| SceneRenderError::Allocation)?;
        for model in models {
            request.check()?;
            let material = model.material();
            // Genetica's standard shader has only commented code under
            // AMBIENT_OCCLUSION. Import 3D sets that flag by default.
            if request.uv_light.is_none()
                && (material.are_shadows_enabled()
                    || material.is_environment_enabled()
                    || material.is_emissive_enabled()
                    || material.reflectivity() != 0.
                    || material.refractivity() != 0.
                    || material.has_fresnel_effect())
            {
                return Err(SceneRenderError::UnsupportedFeature(
                    "advanced material/shadow/environment rendering",
                ));
            }
            let temporary = admission::Temporary::begin()?;
            let bytes = mesh::vertices::<E>(&model, request)?;
            if bytes.len() as u64 > self.device.limits().max_buffer_size {
                return Err(SceneRenderError::InvalidMesh);
            }
            let count =
                u32::try_from(bytes.len() / 44).map_err(|_| SceneRenderError::InvalidMesh)?;
            // The CPU vertex buffer is still alive while the GPU buffer is
            // created; its temporary scope and this retained charge overlap.
            reservation.add(bytes.len() as u64)?;
            let vertex = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Scene independent corner channels"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                });
            drop(bytes);
            drop(temporary);
            // avv.g groups by the actual ad7 identity, preserving first-seen group order.
            let group_ix = if let Some(ix) = groups.iter().position(|group| {
                request.uv_light.is_some() || group.material.same_reference(&material)
            }) {
                ix
            } else {
                let map = if request.uv_light.is_some() {
                    shadow_map.clone()
                } else {
                    let map = material.map();
                    if map.is_some() && material.map_parts() != [1, 1] {
                        return Err(SceneRenderError::UnsupportedFeature(
                            "split material color maps",
                        ));
                    }
                    map
                };
                let (texture, staging, row_pitch, textured, _texture_bytes) =
                    self.material_texture(map.as_ref(), request, reservation)?;
                groups.push(MaterialGroup {
                    material,
                    texture,
                    staging,
                    row_pitch,
                    textured,
                    meshes: Vec::new(),
                });
                groups.len() - 1
            };
            groups[group_ix]
                .meshes
                .try_reserve(1)
                .map_err(|_| SceneRenderError::Allocation)?;
            groups[group_ix].meshes.push(GpuMesh { vertex, count });
        }
        request.check()?;
        Ok(groups)
    }
    #[allow(clippy::too_many_arguments)] // One actual render pass and its retained resources.
    fn draw_lighting<E: 'static>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Scene,
        camera: &SceneCamera<E>,
        request: &SceneRenderRequest<'_>,
        groups: &[MaterialGroup],
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        previous: &wgpu::TextureView,
        is_previous: bool,
        direct: bool,
        reservation: &admission::Reservation,
    ) -> Result<(), SceneRenderError<E>> {
        let quantize_source_rgb = !direct
            && request.uv_light.is_none()
            && !request.uv_material_camera
            && matches!(self.profile.layers, SceneLayerColor::GeneticaOpenGl8);
        let mut bindings = Vec::new();
        bindings
            .try_reserve_exact(groups.len())
            .map_err(|_| SceneRenderError::Allocation)?;
        for group in groups {
            request.check()?;
            let snapshot = read_camera(camera)?;
            request.check()?;
            let temporary = admission::Temporary::begin()?;
            let light_bytes = if let Some((light, shadow)) = request.uv_light {
                mesh::uv_light(light, shadow, snapshot.light_view)?
            } else {
                mesh::lights(scene, &group.material, snapshot.light_view, request)?
            };
            if light_bytes.len() as u64 > self.device.limits().max_storage_buffer_binding_size {
                return Err(SceneRenderError::InvalidGeometry);
            }
            let frame = mesh::frame::<E>(
                &group.material,
                &snapshot,
                is_previous,
                group.textured,
                request.uv_light.is_some(),
                quantize_source_rgb,
            )?;
            let group_bytes = (light_bytes.len() as u64)
                .checked_add(frame.len() as u64)
                .ok_or(SceneRenderError::InvalidGeometry)?;
            // These per-pass buffers are retained by the encoded work, even
            // after their local handles leave this function.
            reservation.add(group_bytes)?;
            let lights = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Scene actual/default lights"),
                    contents: &light_bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let uniform = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Scene ordered camera/material frame"),
                    contents: &frame,
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            drop(light_bytes);
            drop(frame);
            drop(temporary);
            let color_view = group.texture.create_view(&Default::default());
            bindings.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Scene lighting resources"),
                layout: &self.lighting_bindings,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(previous),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: lights.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.linear_repeat),
                    },
                ],
            }));
        }
        request.check()?;
        self.check_device()?;
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if direct {
                "Source direct final framebuffer"
            } else {
                "Source depth-peeling layer"
            }),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(if direct {
            &self.lighting_direct
        } else {
            &self.lighting_peel
        });
        for (group, binding) in groups.iter().zip(&bindings) {
            pass.set_bind_group(0, binding, &[]);
            for mesh in &group.meshes {
                request.check()?;
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.draw(0..mesh.count, 0..1);
            }
        }
        Ok(())
    }
    pub fn render<E: 'static>(
        &self,
        scene: &Scene,
        camera: &SceneCamera<E>,
        request: &SceneRenderRequest<'_>,
    ) -> Result<SceneFrame, SceneRenderError<E>> {
        request.check()?;
        admission::poll_abandoned();
        request.check()?;
        self.check_device()?;
        // Declared before all frame resources, so its Drop runs after them.
        let reservation = admission::Reservation::begin()?;
        let (width, height) = (request.width, request.height);
        let route = RenderRoute::new(request.layers);
        let limits = self.device.limits();
        if width == 0
            || height == 0
            || width > limits.max_texture_dimension_2d
            || height > limits.max_texture_dimension_2d
            || width > i32::MAX as u32
            || height > i32::MAX as u32
        {
            return Err(SceneRenderError::InvalidGeometry);
        }
        if let RenderRoute::Peel(layers) = route {
            if layers > limits.max_texture_array_layers {
                return Err(SceneRenderError::InvalidGeometry);
            }
        }
        if request.uv_light.is_none() && scene.environment().is_some() {
            return Err(SceneRenderError::UnsupportedFeature(
                "environment texture rendering",
            ));
        }
        let row = width
            .checked_mul(8)
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let pitch = row
            .checked_add(255)
            .ok_or(SceneRenderError::InvalidGeometry)?
            / 256
            * 256;
        let readback_bytes = u64::from(pitch)
            .checked_mul(u64::from(height))
            .ok_or(SceneRenderError::InvalidGeometry)?;
        if readback_bytes > limits.max_buffer_size {
            return Err(SceneRenderError::InvalidGeometry);
        }
        let area = u64::from(width)
            .checked_mul(u64::from(height))
            .ok_or(SceneRenderError::InvalidGeometry)?;
        let per_pixel = match route {
            RenderRoute::Direct => 8 + 4 + 16,
            RenderRoute::Peel(layers) => u64::from(layers)
                .checked_mul(u64::from(self.profile.layer_bytes()) + 4)
                .and_then(|n| n.checked_add(8 + 16))
                .ok_or(SceneRenderError::InvalidGeometry)?,
        };
        // Final normalized target, real depth targets, decoded float pixels, padded
        // readback, and the one-pixel unsampled depth binding used on the first pass.
        let target_bytes = area
            .checked_mul(per_pixel)
            .and_then(|n| n.checked_add(readback_bytes))
            .and_then(|n| n.checked_add(4))
            .ok_or(SceneRenderError::InvalidGeometry)?;
        reservation.raise(target_bytes)?;
        // ca/ahx uploads are complete before the first callback-sensitive camera read.
        let groups = self.upload_meshes(scene, request, &reservation)?;
        let output = self.texture(width, height, 1, wgpu::TextureFormat::Rgba16Unorm, false);
        let output_view = output.create_view(&Default::default());
        let dummy = self.texture(1, 1, 1, wgpu::TextureFormat::Depth32Float, true);
        let dummy_view = dummy.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Mycela scene frame"),
            });
        // Encode material uploads only after all fallible CPU preparation has
        // completed. A later failure drops the unsubmitted encoder and staging.
        for group in &groups {
            if let Some(staging) = &group.staging {
                let size = group.texture.size();
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: staging,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(group.row_pitch),
                            rows_per_image: Some(size.height),
                        },
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &group.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    size,
                );
            }
        }
        match route {
            RenderRoute::Direct => {
                let depth = self.texture(width, height, 1, wgpu::TextureFormat::Depth32Float, true);
                let depth_view = depth.create_view(&Default::default());
                self.draw_lighting(
                    &mut encoder,
                    scene,
                    camera,
                    request,
                    &groups,
                    &output_view,
                    &depth_view,
                    &dummy_view,
                    false,
                    true,
                    &reservation,
                )?;
            }
            RenderRoute::Peel(layers) => {
                let colors =
                    self.texture(width, height, layers, self.profile.layer_format(), false);
                let mut depths = Vec::new();
                let mut depth_views = Vec::new();
                depths
                    .try_reserve_exact(layers as usize)
                    .map_err(|_| SceneRenderError::Allocation)?;
                depth_views
                    .try_reserve_exact(layers as usize)
                    .map_err(|_| SceneRenderError::Allocation)?;
                for _ in 0..layers {
                    request.check()?;
                    let depth = self.texture(width, height, 1, self.profile.depth_format(), true);
                    depth_views.push(depth.create_view(&Default::default()));
                    depths.push(depth);
                }
                for layer in 0..layers {
                    request.check()?;
                    // Nearest first into the last map, then peel toward map zero.
                    let color = colors.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: layers - layer - 1,
                        array_layer_count: Some(1),
                        ..Default::default()
                    });
                    let previous = if layer == 0 {
                        &dummy_view
                    } else {
                        &depth_views[layer as usize - 1]
                    };
                    self.draw_lighting(
                        &mut encoder,
                        scene,
                        camera,
                        request,
                        &groups,
                        &color,
                        &depth_views[layer as usize],
                        previous,
                        layer != 0,
                        false,
                        &reservation,
                    )?;
                }
                request.check()?;
                let layer_view = colors.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                });
                let mut projection = [0u8; 64];
                for (slot, value) in projection
                    .chunks_exact_mut(4)
                    .zip(composite_projection(width, height))
                {
                    slot.copy_from_slice(&value.to_le_bytes());
                }
                reservation.add(64)?;
                let uniform = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("Source Double to Single composite projection"),
                        contents: &projection,
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let blend = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Source unit-UV layer composition"),
                    layout: &self.composite_bindings,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&layer_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.linear_repeat),
                        },
                    ],
                });
                let attachments = [Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Source projected straight-alpha composite"),
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.composite);
                pass.set_bind_group(0, &blend, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Scene normalized16 readback"),
            size: readback_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        request.check()?;
        let (tx, rx) = mpsc::channel();
        let mapped = reservation.readback_callback();
        // Register the readback against this actual command buffer before it is
        // submitted. The reservation waits for both this callback and the GPU
        // completion callback, even if the caller cancels immediately afterward.
        encoder.map_buffer_on_submit(&readback, wgpu::MapMode::Read, .., move |result| {
            mapped();
            let _ = tx.send(result);
        });
        reservation.prepare_submit(&encoder, &self.device);
        let submission = self.queue.submit([encoder.finish()]);
        reservation.submitted(submission);
        loop {
            request.check()?;
            self.check_device()?;
            match rx.try_recv() {
                Ok(Ok(())) => break,
                Ok(Err(error)) => return Err(SceneRenderError::Device(error.to_string())),
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(SceneRenderError::Device(
                        "Scene readback callback disconnected".into(),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            match self.device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_millis(10)),
            }) {
                Ok(_) | Err(wgpu::PollError::Timeout) => {}
                Err(error) => return Err(SceneRenderError::Device(error.to_string())),
            }
        }
        self.check_device()?;
        request.check()?;
        let mapped = readback.slice(..).get_mapped_range();
        let mut pixels = Vec::new();
        let pixel_count = usize::try_from(area).map_err(|_| SceneRenderError::InvalidGeometry)?;
        pixels
            .try_reserve_exact(pixel_count)
            .map_err(|_| SceneRenderError::Allocation)?;
        for y in 0..height as usize {
            request.check()?;
            // The ordinary scene worker reads OpenGL color rows without
            // flipping them. UV Light follows its own camera path.
            let y = if request.uv_light.is_none() {
                height as usize - 1 - y
            } else {
                y
            };
            let bytes = &mapped[y * pitch as usize..y * pitch as usize + row as usize];
            for (ix, pixel) in bytes.chunks_exact(8).enumerate() {
                if ix % 1024 == 0 {
                    request.check()?;
                }
                pixels.push(std::array::from_fn(|ch| {
                    f32::from(u16::from_le_bytes(
                        pixel[ch * 2..ch * 2 + 2]
                            .try_into()
                            .expect("two-byte channel"),
                    )) / 65535.
                }));
            }
        }
        drop(mapped);
        readback.unmap();
        request.check()?;
        Ok(SceneFrame {
            width,
            height,
            pixels,
        })
    }
}
fn lighting_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    let attributes = wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x3,3=>Float32x2];
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: 44,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    };
    let targets = [Some(wgpu::ColorTargetState {
        format: color,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[vertex_layout],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}
fn admit<E>(bytes: u64) -> Result<(), SceneRenderError<E>> {
    admission::admit_extra(bytes)
}

#[cfg(test)]
mod laws;

mod shader;
pub use shader::ShaderRenderer;

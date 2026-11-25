//! A fullscreen fragment pass for Mycela Shader graph nodes.
use super::SceneFrame;
use half::f16;
use mycela_engine::{ImageGeometry, ShaderProgram, SharedImageMap};
use std::{
    borrow::Cow,
    sync::mpsc,
    time::{Duration, Instant},
};
use wgpu as gpu;

const FORMAT: gpu::TextureFormat = gpu::TextureFormat::Rgba16Float;

pub struct ShaderRenderer {
    device: gpu::Device,
    queue: gpu::Queue,
}

impl ShaderRenderer {
    pub async fn new() -> Result<Self, String> {
        let instance = gpu::Instance::new(gpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&gpu::RequestAdapterOptions::default())
            .await
            .map_err(|error| error.to_string())?;
        let (device, queue) = adapter
            .request_device(&gpu::DeviceDescriptor::default())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self { device, queue })
    }

    pub async fn render(
        &self,
        program: &ShaderProgram,
        inputs: &[Option<SharedImageMap>],
        geometry: ImageGeometry,
        time: f64,
        cancelled: &dyn Fn() -> bool,
        deadline: Instant,
    ) -> Result<SharedImageMap, String> {
        let ready = || {
            if cancelled() {
                Err("Shader render cancelled.".to_owned())
            } else if Instant::now() >= deadline {
                Err("Shader render timed out.".to_owned())
            } else {
                Ok(())
            }
        };
        ready()?;
        let size = self.size(geometry)?;
        let device = &self.device;
        let validation = device.push_error_scope(gpu::ErrorFilter::Validation);
        let allocation = device.push_error_scope(gpu::ErrorFilter::OutOfMemory);
        let module = device.create_shader_module(gpu::ShaderModuleDescriptor {
            label: Some("Mycela Shader"),
            source: gpu::ShaderSource::Wgsl(Cow::Borrowed(program.wgsl())),
        });
        let uniform = gpu::util::DeviceExt::create_buffer_init(
            device,
            &gpu::util::BufferInitDescriptor {
                label: Some("Shader built-ins"),
                contents: &[size.width as f32, size.height as f32, time as f32, 0.]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
                usage: gpu::BufferUsages::UNIFORM,
            },
        );
        let sampler = device.create_sampler(&gpu::SamplerDescriptor {
            address_mode_u: gpu::AddressMode::Repeat,
            address_mode_v: gpu::AddressMode::Repeat,
            mag_filter: gpu::FilterMode::Linear,
            min_filter: gpu::FilterMode::Linear,
            ..Default::default()
        });
        let globals = device.create_bind_group_layout(&gpu::BindGroupLayoutDescriptor {
            label: Some("Shader built-ins"),
            entries: &[
                binding(
                    0,
                    gpu::BindingType::Buffer {
                        ty: gpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                binding(
                    1,
                    gpu::BindingType::Sampler(gpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let globals_group = device.create_bind_group(&gpu::BindGroupDescriptor {
            label: Some("Shader built-ins"),
            layout: &globals,
            entries: &[
                gpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                gpu::BindGroupEntry {
                    binding: 1,
                    resource: gpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let input_layout = device.create_bind_group_layout(&gpu::BindGroupLayoutDescriptor {
            label: Some("Shader graph inputs"),
            entries: &program
                .inputs()
                .iter()
                .map(|input| {
                    binding(
                        input.binding(),
                        gpu::BindingType::Texture {
                            sample_type: gpu::TextureSampleType::Float { filterable: true },
                            view_dimension: gpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                    )
                })
                .collect::<Vec<_>>(),
        });
        let layout = device.create_pipeline_layout(&gpu::PipelineLayoutDescriptor {
            label: Some("Shader layout"),
            bind_group_layouts: &[Some(&globals), Some(&input_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&gpu::RenderPipelineDescriptor {
            label: Some("Shader fragment pass"),
            layout: Some(&layout),
            vertex: gpu::VertexState {
                module: &module,
                entry_point: Some("mycela_vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(gpu::FragmentState {
                module: &module,
                entry_point: Some("mycela_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let textures = program
            .inputs()
            .iter()
            .enumerate()
            .map(|(index, _)| {
                ready()?;
                self.upload(inputs.get(index).and_then(Option::as_ref), &ready)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let images = device.create_bind_group(&gpu::BindGroupDescriptor {
            label: Some("Shader images"),
            layout: &input_layout,
            entries: &program
                .inputs()
                .iter()
                .zip(&textures)
                .map(|(input, view)| gpu::BindGroupEntry {
                    binding: input.binding(),
                    resource: gpu::BindingResource::TextureView(view),
                })
                .collect::<Vec<_>>(),
        });
        let target = self.texture(
            size,
            gpu::TextureUsages::RENDER_ATTACHMENT | gpu::TextureUsages::COPY_SRC,
        );
        let target_view = target.create_view(&Default::default());
        let row = size.width * 8;
        let pitch =
            row.div_ceil(gpu::COPY_BYTES_PER_ROW_ALIGNMENT) * gpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let byte_count = u64::from(pitch) * u64::from(size.height);
        if byte_count > device.limits().max_buffer_size {
            return Err("Shader output exceeds the GPU readback limit.".into());
        }
        let readback = device.create_buffer(&gpu::BufferDescriptor {
            label: Some("Shader image readback"),
            size: byte_count,
            usage: gpu::BufferUsages::COPY_DST | gpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&gpu::RenderPassDescriptor {
                label: Some("Render Shader"),
                color_attachments: &[Some(gpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: gpu::Operations {
                        load: gpu::LoadOp::Clear(gpu::Color::TRANSPARENT),
                        store: gpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &globals_group, &[]);
            pass.set_bind_group(1, &images, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            gpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: gpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        let (tx, rx) = mpsc::channel();
        encoder.map_buffer_on_submit(&readback, gpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        if let Some(error) = allocation.pop().await.or(validation.pop().await) {
            return Err(error.to_string());
        }
        ready()?;
        self.queue.submit([encoder.finish()]);
        loop {
            ready()?;
            match rx.try_recv() {
                Ok(result) => {
                    result.map_err(|error| error.to_string())?;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("Shader readback ended early.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            match device.poll(gpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_millis(10)),
            }) {
                Ok(_) | Err(gpu::PollError::Timeout) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        let bytes = readback.slice(..).get_mapped_range();
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(size.width as usize * size.height as usize)
            .map_err(|error| error.to_string())?;
        for line in bytes.chunks_exact(pitch as usize) {
            ready()?;
            for pixel in line[..row as usize].chunks_exact(8) {
                pixels.push(std::array::from_fn(|channel| {
                    let bits = u16::from_le_bytes([pixel[channel * 2], pixel[channel * 2 + 1]]);
                    let value = f16::from_bits(bits).to_f32();
                    if value.is_finite() { value } else { 0.0 }
                }));
            }
        }
        drop(bytes);
        readback.unmap();
        SceneFrame {
            width: size.width,
            height: size.height,
            pixels,
        }
        .into_image()
        .map_err(|error| error.to_string())
    }

    fn size(&self, geometry: ImageGeometry) -> Result<gpu::Extent3d, String> {
        let max = self.device.limits().max_texture_dimension_2d as i32;
        if !(1..=max).contains(&geometry.width()) || !(1..=max).contains(&geometry.height()) {
            return Err(format!("Shader dimensions must be between 1 and {max}."));
        }
        Ok(gpu::Extent3d {
            width: geometry.width() as u32,
            height: geometry.height() as u32,
            depth_or_array_layers: 1,
        })
    }

    fn texture(&self, size: gpu::Extent3d, usage: gpu::TextureUsages) -> gpu::Texture {
        self.device.create_texture(&gpu::TextureDescriptor {
            label: Some("Shader image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    }

    fn upload(
        &self,
        input: Option<&SharedImageMap>,
        ready: &impl Fn() -> Result<(), String>,
    ) -> Result<gpu::TextureView, String> {
        let mut bytes = Vec::new();
        let size = if let Some(image) = input {
            let size = self.size(image.geometry())?;
            bytes
                .try_reserve_exact(size.width as usize * size.height as usize * 8)
                .map_err(|error| error.to_string())?;
            let channels = (0..4)
                .map(|index| image.materialize(index).map_err(|error| error.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            for y in 0..size.height as usize {
                ready()?;
                for x in 0..size.width as usize {
                    for (index, channel) in channels.iter().enumerate() {
                        let value = channel
                            .read(y * size.width as usize + x)
                            .map_err(|error| error.to_string())?;
                        let value = if index == 3 { 1. - value } else { value };
                        bytes.extend(f16::from_f32(value).to_bits().to_le_bytes());
                    }
                }
            }
            size
        } else {
            bytes.resize(8, 0);
            self.size(ImageGeometry::new(1, 1))?
        };
        let texture = self.texture(
            size,
            gpu::TextureUsages::COPY_DST | gpu::TextureUsages::TEXTURE_BINDING,
        );
        self.queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            gpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 8),
                rows_per_image: Some(size.height),
            },
            size,
        );
        Ok(texture.create_view(&Default::default()))
    }
}

fn binding(binding: u32, ty: gpu::BindingType) -> gpu::BindGroupLayoutEntry {
    gpu::BindGroupLayoutEntry {
        binding,
        visibility: gpu::ShaderStages::FRAGMENT,
        ty,
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a working wgpu adapter"]
    fn shader_renders_named_input_and_writes_swizzles_on_the_device() {
        let source = r#"
requires swizzle_assignment;
@group(1) @binding(7) var base: texture_2d<f32>;
var<private> state = vec4f(0.0, 0.125, 0.0, 1.0);
var<private> index_calls: u32 = 0u;
struct Pixel { rg: vec4f }
fn change_state() -> vec2f {
    state = vec4f(0.75, 0.75, 0.125, 0.25);
    return vec2f(0.125);
}
fn next_index() -> u32 {
    index_calls += 1u;
    return index_calls - 1u;
}
fn shade(uv: vec2f) -> vec4f {
    var color = sample_input(base, uv);
    color.rg = vec2f(color.g, time);
    color.ba += vec2f(texel_size.x, -0.25);
    state.rg += change_state();
    var step = vec2u();
    for (step.xy = vec2u(0u, 1u); step.x < 2u; step.xy += vec2u(1u)) {
        if step.x == 0u { continue; }
        color += state;
    }
    var layer = Pixel(vec4f());
    let pointer = &layer.rg;
    pointer.rgba.gb = vec2f(0.125, 0.25);
    (layer.rg.agr).z = 0.125;
    layer.rg.ab[next_index()] = 0.25;
    if index_calls != 1u { return vec4f(); }
    return color + layer.rg;
}
"#;
        let renderer = pollster::block_on(ShaderRenderer::new()).unwrap();
        let program = ShaderProgram::compile(source).unwrap();
        let input = SceneFrame {
            width: 1,
            height: 1,
            pixels: vec![[0.125, 0.5, 0.25, 0.75]],
        }
        .into_image()
        .unwrap();
        let output = pollster::block_on(renderer.render(
            &program,
            &[Some(input)],
            ImageGeometry::new(8, 4),
            0.375,
            &|| false,
            Instant::now() + Duration::from_secs(15),
        ))
        .unwrap();
        for (channel, expected) in [0.75, 0.75, 0.75, 0.0].into_iter().enumerate() {
            let values = output.materialize(channel as i32).unwrap();
            assert!(
                values.values().iter().all(|value| *value == expected),
                "channel {channel}: {:?}, expected {expected}",
                values.read(0)
            );
        }
    }
}

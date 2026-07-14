//! Scene-space blur used by modal focus effects.
//!
//! The normal launcher scene is rendered into `scene_texture`. When a neutral
//! backdrop view requests `scene_blur`, that completed scene is blurred through
//! a three-level Dual-Kawase pyramid and composited back through the backdrop's
//! rounded rectangle. Modal glass and modal content render after this pass.

use std::num::NonZeroU64;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct FocusBlurUniforms {
    /// (viewport width, viewport height, blur mix, corner radius)
    viewport_mix_radius: [f32; 4],
    /// (center x, center y, half width, half height)
    frame: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
pub(super) struct FocusBlurParams {
    pub viewport: (u32, u32),
    pub center: [f32; 2],
    pub half_size: [f32; 2],
    pub radius: f32,
    pub strength: f32,
}

pub(super) struct FocusBlurRenderer {
    scene_texture: wgpu::Texture,
    scene_view: wgpu::TextureView,
    blur_texture: wgpu::Texture,
    blur_view: wgpu::TextureView,
    blur_levels: [(wgpu::Texture, wgpu::TextureView); 3],
    sampler: wgpu::Sampler,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    blur_down_bind_groups: [wgpu::BindGroup; 3],
    blur_up_bind_groups: [wgpu::BindGroup; 3],
    composite_bind_group_layout: wgpu::BindGroupLayout,
    composite_bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    blur_downsample_pipeline: wgpu::RenderPipeline,
    blur_upsample_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    size: (u32, u32),
}

impl FocusBlurRenderer {
    pub(super) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("focus blur sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let blur_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("focus blur bgl"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let composite_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("focus blur composite bgl"),
                entries: &[
                    texture_entry(0),
                    texture_entry(1),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<FocusBlurUniforms>() as u64,
                            ),
                        },
                        count: None,
                    },
                ],
            });

        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("focus blur downsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_downsample.wgsl").into(),
            ),
        });
        let upsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("focus blur upsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_upsample.wgsl").into(),
            ),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("focus blur composite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_focus_blur.wgsl").into()),
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("focus blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_bind_group_layout)],
            immediate_size: 0,
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("focus blur composite pipeline layout"),
                bind_group_layouts: &[Some(&composite_bind_group_layout)],
                immediate_size: 0,
            });
        let blur_downsample_pipeline = fullscreen_pipeline(
            device,
            "focus blur downsample pipeline",
            &blur_pipeline_layout,
            &blur_shader,
            format,
        );
        let blur_upsample_pipeline = fullscreen_pipeline(
            device,
            "focus blur upsample pipeline",
            &blur_pipeline_layout,
            &upsample_shader,
            format,
        );
        let composite_pipeline = fullscreen_pipeline(
            device,
            "focus blur composite pipeline",
            &composite_pipeline_layout,
            &composite_shader,
            format,
        );
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("focus blur uniform buffer"),
            size: std::mem::size_of::<FocusBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (scene_texture, scene_view) =
            create_texture(device, "focus scene texture", width, height, format);
        let (blur_texture, blur_view) =
            create_texture(device, "focus blur texture", width, height, format);
        let blur_levels = create_blur_levels(device, width, height, format);
        let blur_down_bind_groups = create_down_bind_groups(
            device,
            &blur_bind_group_layout,
            &scene_view,
            &blur_levels,
            &sampler,
        );
        let blur_up_bind_groups =
            create_up_bind_groups(device, &blur_bind_group_layout, &blur_levels, &sampler);
        let composite_bind_group = create_composite_bind_group(
            device,
            &composite_bind_group_layout,
            &scene_view,
            &blur_view,
            &sampler,
            &uniform_buffer,
        );

        Self {
            scene_texture,
            scene_view,
            blur_texture,
            blur_view,
            blur_levels,
            sampler,
            blur_bind_group_layout,
            blur_down_bind_groups,
            blur_up_bind_groups,
            composite_bind_group_layout,
            composite_bind_group,
            uniform_buffer,
            blur_downsample_pipeline,
            blur_upsample_pipeline,
            composite_pipeline,
            format,
            size: (width.max(1), height.max(1)),
        }
    }

    pub(super) fn scene_view(&self) -> &wgpu::TextureView {
        &self.scene_view
    }

    pub(super) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.size == (width, height) {
            return;
        }
        let (scene_texture, scene_view) =
            create_texture(device, "focus scene texture", width, height, self.format);
        let (blur_texture, blur_view) =
            create_texture(device, "focus blur texture", width, height, self.format);
        let blur_levels = create_blur_levels(device, width, height, self.format);
        self.blur_down_bind_groups = create_down_bind_groups(
            device,
            &self.blur_bind_group_layout,
            &scene_view,
            &blur_levels,
            &self.sampler,
        );
        self.blur_up_bind_groups = create_up_bind_groups(
            device,
            &self.blur_bind_group_layout,
            &blur_levels,
            &self.sampler,
        );
        self.composite_bind_group = create_composite_bind_group(
            device,
            &self.composite_bind_group_layout,
            &scene_view,
            &blur_view,
            &self.sampler,
            &self.uniform_buffer,
        );
        self.scene_texture = scene_texture;
        self.scene_view = scene_view;
        self.blur_texture = blur_texture;
        self.blur_view = blur_view;
        self.blur_levels = blur_levels;
        self.size = (width, height);
    }

    /// Run a strong three-level Dual-Kawase blur over the completed lower scene.
    pub(super) fn blur(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut super::gpu_profile::GpuProfilerState,
    ) {
        const DOWN_LABELS: [&str; 3] = [
            "focus_blur_downsample_1",
            "focus_blur_downsample_2",
            "focus_blur_downsample_3",
        ];
        const UP_LABELS: [&str; 3] = [
            "focus_blur_upsample_1",
            "focus_blur_upsample_2",
            "focus_blur_upsample_3",
        ];
        for (i, label) in DOWN_LABELS.into_iter().enumerate() {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("focus blur downsample encoder"),
            });
            let profile_scope = profiler.begin(label, &mut encoder);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("focus blur downsample pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.blur_levels[i].1,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blur_downsample_pipeline);
                pass.set_bind_group(0, &self.blur_down_bind_groups[i], &[]);
                pass.draw(0..3, 0..1);
            }
            profiler.end(&mut encoder, profile_scope);
            profiler.resolve(&mut encoder);
            queue.submit(std::iter::once(encoder.finish()));
        }
        for (j, label) in UP_LABELS.into_iter().enumerate() {
            let destination = if j == 2 {
                &self.blur_view
            } else {
                &self.blur_levels[1 - j].1
            };
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("focus blur upsample encoder"),
            });
            let profile_scope = profiler.begin(label, &mut encoder);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("focus blur upsample pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: destination,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.blur_upsample_pipeline);
                pass.set_bind_group(0, &self.blur_up_bind_groups[j], &[]);
                pass.draw(0..3, 0..1);
            }
            profiler.end(&mut encoder, profile_scope);
            profiler.resolve(&mut encoder);
            queue.submit(std::iter::once(encoder.finish()));
        }
    }

    pub(super) fn composite(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        params: FocusBlurParams,
    ) {
        let uniforms = FocusBlurUniforms {
            viewport_mix_radius: [
                params.viewport.0.max(1) as f32,
                params.viewport.1.max(1) as f32,
                params.strength.clamp(0.0, 1.0),
                params.radius.max(0.0),
            ],
            frame: [
                params.center[0],
                params.center[1],
                params.half_size[0].max(0.0),
                params.half_size[1].max(0.0),
            ],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("focus blur composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &self.composite_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_blur_levels(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> [(wgpu::Texture, wgpu::TextureView); 3] {
    std::array::from_fn(|index| {
        let divisor = 1u32 << (index + 1);
        create_texture(
            device,
            "focus blur pyramid level",
            (width / divisor).max(1),
            (height / divisor).max(1),
            format,
        )
    })
}

fn blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("focus blur bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_down_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene: &wgpu::TextureView,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    sampler: &wgpu::Sampler,
) -> [wgpu::BindGroup; 3] {
    [
        blur_bind_group(device, layout, scene, sampler),
        blur_bind_group(device, layout, &levels[0].1, sampler),
        blur_bind_group(device, layout, &levels[1].1, sampler),
    ]
}

fn create_up_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    sampler: &wgpu::Sampler,
) -> [wgpu::BindGroup; 3] {
    [
        blur_bind_group(device, layout, &levels[2].1, sampler),
        blur_bind_group(device, layout, &levels[1].1, sampler),
        blur_bind_group(device, layout, &levels[0].1, sampler),
    ]
}

fn create_composite_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    scene: &wgpu::TextureView,
    blurred: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("focus blur composite bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(scene),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(blurred),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_blur_uniform_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<FocusBlurUniforms>(), 32);
    }
}

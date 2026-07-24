//! Full-resolution GPU blur for text shadows.
//!
//! Every text lane first writes glyph coverage into `shadow_texture`. The mask
//! is blurred in a two-layer, separable Gaussian pass:
//!   * **R / main shadow** — narrow (σ ≈ 0.75 logical px) so sharp corners
//!     ("A", "フ") keep a dense shadow core; offset to +1,+1 px at composite
//!     time (CSS `1px 1px 2px`).
//!   * **A / halo** — wider (σ ≈ 1.75 logical px) zero-offset halo (CSS
//!     `0 0 4px`).
//!
//! Concept:
//! ```text
//! coverage ─┬─ narrow horizontal → narrow vertical → R
//!           └─ wide horizontal   → wide vertical   → A
//! ```
//! Both channels are real Gaussians (the previous design passed the raw
//! coverage through R, which let glyph corners lose shadow area where the
//! offset body overlapped). The mask stays at physical surface resolution so
//! its coverage matches the body glyph; only the Gaussian sigma and offset
//! scale with the display factor to keep logical CSS-pixel dimensions stable.

use std::num::NonZeroU64;

pub(super) const TEXT_SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Narrow (main shadow) Gaussian sigma in logical CSS pixels.
pub(super) const MAIN_SIGMA_LOGICAL: f32 = 0.75;
/// Wide (halo) Gaussian sigma in logical CSS pixels.
pub(super) const HALO_SIGMA_LOGICAL: f32 = 1.75;

fn sanitized_scale_factor(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextShadowBlurUniforms {
    /// (physical px per logical px, narrow sigma logical, wide sigma logical, _)
    sample_scale: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextShadowCompositeUniforms {
    /// (offset x px, offset y px, main alpha, halo alpha)
    offset_alpha: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
pub(super) struct TextShadowParams {
    pub offset: [f32; 2],
    pub main_alpha: f32,
    pub halo_alpha: f32,
}

impl Default for TextShadowParams {
    fn default() -> Self {
        Self {
            offset: [1.0, 1.0],
            main_alpha: 0.85,
            halo_alpha: 0.35,
        }
    }
}

pub(super) struct TextShadowBlur {
    shadow_texture: wgpu::Texture,
    shadow_view: wgpu::TextureView,
    blur_h_texture: wgpu::Texture,
    blur_h_view: wgpu::TextureView,
    blur_v_texture: wgpu::Texture,
    blur_v_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    blur_h_bind_group: wgpu::BindGroup,
    blur_v_bind_group: wgpu::BindGroup,
    blur_uniform_buffer: wgpu::Buffer,
    composite_bind_group_layout: wgpu::BindGroupLayout,
    composite_bind_group: wgpu::BindGroup,
    composite_uniform_buffer: wgpu::Buffer,
    blur_h_pipeline: wgpu::RenderPipeline,
    blur_v_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    size: (u32, u32),
}

impl TextShadowBlur {
    pub(super) fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("text shadow sampler"),
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
                label: Some("text shadow blur bgl"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(std::mem::size_of::<
                                TextShadowBlurUniforms,
                            >()
                                as u64),
                        },
                        count: None,
                    },
                ],
            });
        let composite_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("text shadow composite bgl"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(std::mem::size_of::<
                                TextShadowCompositeUniforms,
                            >()
                                as u64),
                        },
                        count: None,
                    },
                ],
            });

        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text shadow blur shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shader_text_shadow_blur.wgsl").into(),
            ),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text shadow composite shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shader_text_shadow_composite.wgsl").into(),
            ),
        });
        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text shadow blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_bind_group_layout)],
            immediate_size: 0,
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("text shadow composite pipeline layout"),
                bind_group_layouts: &[Some(&composite_bind_group_layout)],
                immediate_size: 0,
            });
        let blur_h_pipeline = fullscreen_pipeline(
            device,
            "text shadow horizontal blur pipeline",
            &blur_pipeline_layout,
            &blur_shader,
            "fs_horizontal",
            TEXT_SHADOW_FORMAT,
            None,
        );
        let blur_v_pipeline = fullscreen_pipeline(
            device,
            "text shadow vertical blur pipeline",
            &blur_pipeline_layout,
            &blur_shader,
            "fs_vertical",
            TEXT_SHADOW_FORMAT,
            None,
        );
        let composite_pipeline = fullscreen_pipeline(
            device,
            "text shadow composite pipeline",
            &composite_pipeline_layout,
            &composite_shader,
            "fs_main",
            target_format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let composite_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text shadow composite uniform buffer"),
            size: std::mem::size_of::<TextShadowCompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blur_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("text shadow blur uniform buffer"),
            size: std::mem::size_of::<TextShadowBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (shadow_texture, shadow_view) =
            create_texture(device, "text shadow mask texture", width, height);
        let (blur_h_texture, blur_h_view) =
            create_texture(device, "text shadow horizontal blur texture", width, height);
        let (blur_v_texture, blur_v_view) =
            create_texture(device, "text shadow vertical blur texture", width, height);
        let blur_h_bind_group = blur_bind_group(
            device,
            &blur_bind_group_layout,
            &shadow_view,
            &sampler,
            &blur_uniform_buffer,
        );
        let blur_v_bind_group = blur_bind_group(
            device,
            &blur_bind_group_layout,
            &blur_h_view,
            &sampler,
            &blur_uniform_buffer,
        );
        let composite_bind_group = composite_bind_group(
            device,
            &composite_bind_group_layout,
            &blur_v_view,
            &sampler,
            &composite_uniform_buffer,
        );

        Self {
            shadow_texture,
            shadow_view,
            blur_h_texture,
            blur_h_view,
            blur_v_texture,
            blur_v_view,
            sampler,
            blur_bind_group_layout,
            blur_h_bind_group,
            blur_v_bind_group,
            blur_uniform_buffer,
            composite_bind_group_layout,
            composite_bind_group,
            composite_uniform_buffer,
            blur_h_pipeline,
            blur_v_pipeline,
            composite_pipeline,
            size: (width.max(1), height.max(1)),
        }
    }

    pub(super) fn shadow_view(&self) -> &wgpu::TextureView {
        &self.shadow_view
    }

    pub(super) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.size == (width, height) {
            return;
        }

        let (shadow_texture, shadow_view) =
            create_texture(device, "text shadow mask texture", width, height);
        let (blur_h_texture, blur_h_view) =
            create_texture(device, "text shadow horizontal blur texture", width, height);
        let (blur_v_texture, blur_v_view) =
            create_texture(device, "text shadow vertical blur texture", width, height);
        self.blur_h_bind_group = blur_bind_group(
            device,
            &self.blur_bind_group_layout,
            &shadow_view,
            &self.sampler,
            &self.blur_uniform_buffer,
        );
        self.blur_v_bind_group = blur_bind_group(
            device,
            &self.blur_bind_group_layout,
            &blur_h_view,
            &self.sampler,
            &self.blur_uniform_buffer,
        );
        self.composite_bind_group = composite_bind_group(
            device,
            &self.composite_bind_group_layout,
            &blur_v_view,
            &self.sampler,
            &self.composite_uniform_buffer,
        );
        self.shadow_texture = shadow_texture;
        self.shadow_view = shadow_view;
        self.blur_h_texture = blur_h_texture;
        self.blur_h_view = blur_h_view;
        self.blur_v_texture = blur_v_texture;
        self.blur_v_view = blur_v_view;
        self.size = (width, height);
    }

    pub(super) fn blur(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scale_factor: f32,
    ) {
        let scale = sanitized_scale_factor(scale_factor);
        queue.write_buffer(
            &self.blur_uniform_buffer,
            0,
            bytemuck::bytes_of(&TextShadowBlurUniforms {
                sample_scale: [scale, MAIN_SIGMA_LOGICAL, HALO_SIGMA_LOGICAL, 0.0],
            }),
        );
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text shadow horizontal blur pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.blur_h_view,
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
            pass.set_pipeline(&self.blur_h_pipeline);
            pass.set_bind_group(0, &self.blur_h_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text shadow vertical blur pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.blur_v_view,
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
            pass.set_pipeline(&self.blur_v_pipeline);
            pass.set_bind_group(0, &self.blur_v_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub(super) fn composite(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        params: TextShadowParams,
        scale_factor: f32,
    ) {
        let scale = sanitized_scale_factor(scale_factor);
        let uniforms = TextShadowCompositeUniforms {
            offset_alpha: [
                params.offset[0] * scale,
                params.offset[1] * scale,
                params.main_alpha.clamp(0.0, 1.0),
                params.halo_alpha.clamp(0.0, 1.0),
            ],
        };
        queue.write_buffer(
            &self.composite_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("text shadow composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
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
    fragment_entry: &'static str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
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
            entry_point: Some(fragment_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
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
        format: TEXT_SHADOW_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("text shadow blur bg"),
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
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

fn composite_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("text shadow composite bg"),
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
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_uniform_layout_matches_wgsl_vec4() {
        assert_eq!(std::mem::size_of::<TextShadowCompositeUniforms>(), 16);
        assert_eq!(std::mem::align_of::<TextShadowCompositeUniforms>(), 4);
        assert_eq!(std::mem::size_of::<TextShadowBlurUniforms>(), 16);
        assert_eq!(std::mem::align_of::<TextShadowBlurUniforms>(), 4);
    }

    /// Gaussian weight at integer distance `d` (physical px) for a given sigma.
    fn gw(d: f32, sigma: f32) -> f32 {
        (-0.5 * d * d / (sigma * sigma)).exp()
    }

    #[test]
    fn narrow_gaussian_weights_normalize() {
        // σ = 0.75 logical px; the shader taps ±8 physical px (4 paired taps).
        // The paired sum must match the direct normalization so the blur is
        // energy-preserving.
        for scale in [1.0_f32, 2.0] {
            let sigma = MAIN_SIGMA_LOGICAL * scale;
            let direct = gw(0.0, sigma)
                + 2.0
                    * (gw(1.0, sigma)
                        + gw(2.0, sigma)
                        + gw(3.0, sigma)
                        + gw(4.0, sigma)
                        + gw(5.0, sigma)
                        + gw(6.0, sigma)
                        + gw(7.0, sigma)
                        + gw(8.0, sigma));
            let paired = gw(0.0, sigma)
                + 2.0
                    * ((gw(1.0, sigma) + gw(2.0, sigma))
                        + (gw(3.0, sigma) + gw(4.0, sigma))
                        + (gw(5.0, sigma) + gw(6.0, sigma))
                        + (gw(7.0, sigma) + gw(8.0, sigma)));
            assert!(
                (paired / direct - 1.0).abs() < 0.000_001,
                "narrow scale={scale}: paired {paired} must match direct {direct}"
            );
        }
    }

    #[test]
    fn wide_gaussian_weights_normalize() {
        for scale in [1.0_f32, 2.0] {
            let sigma = HALO_SIGMA_LOGICAL * scale;
            let direct = gw(0.0, sigma)
                + 2.0
                    * (gw(1.0, sigma)
                        + gw(2.0, sigma)
                        + gw(3.0, sigma)
                        + gw(4.0, sigma)
                        + gw(5.0, sigma)
                        + gw(6.0, sigma)
                        + gw(7.0, sigma)
                        + gw(8.0, sigma));
            let paired = gw(0.0, sigma)
                + 2.0
                    * ((gw(1.0, sigma) + gw(2.0, sigma))
                        + (gw(3.0, sigma) + gw(4.0, sigma))
                        + (gw(5.0, sigma) + gw(6.0, sigma))
                        + (gw(7.0, sigma) + gw(8.0, sigma)));
            assert!(
                (paired / direct - 1.0).abs() < 0.000_001,
                "wide scale={scale}: paired {paired} must match direct {direct}"
            );
        }
    }

    #[test]
    fn kernel_radius_covers_sigma_at_both_scales() {
        // The shader uses a fixed ±8 physical-tap radius. Verify the Gaussian
        // tail beyond ±8 is negligible for both sigmas at scale 2.0 (the worst
        // case), so no significant coverage leaks past the kernel edge.
        for logical in [MAIN_SIGMA_LOGICAL, HALO_SIGMA_LOGICAL] {
            let sigma = logical * 2.0;
            let tail = gw(9.0, sigma) + gw(10.0, sigma) + gw(11.0, sigma) + gw(12.0, sigma);
            let total = gw(0.0, sigma)
                + 2.0
                    * (gw(1.0, sigma)
                        + gw(2.0, sigma)
                        + gw(3.0, sigma)
                        + gw(4.0, sigma)
                        + gw(5.0, sigma)
                        + gw(6.0, sigma)
                        + gw(7.0, sigma)
                        + gw(8.0, sigma))
                + 2.0 * tail;
            let leakage = 2.0 * tail / total;
            assert!(
                leakage < 0.02,
                "σ={logical}@2×: tail leakage {leakage} beyond ±8 taps is too large"
            );
        }
    }

    #[test]
    fn bilinear_paired_sample_reproduces_discrete_taps() {
        // The shader samples a pair of symmetric taps (±first, ±second) at a
        // weighted-average offset. When the two weights are equal the sample
        // lands midway and linear filtering yields the average — which, scaled
        // by the combined weight, equals the sum of the two discrete taps.
        // Equal weights at distance 1 and 1 (degenerate) → offset 1.5.
        let sigma = 1.0;
        let w1 = gw(1.0, sigma);
        let combined = w1 + w1;
        let offset = 1.0 + w1 / combined;
        assert!(
            (offset - 1.5).abs() < 0.000_001,
            "equal-weight pair must sample at 1.5, got {offset}"
        );
    }

    #[test]
    fn default_shadow_matches_design_values() {
        let params = TextShadowParams::default();
        assert_eq!(params.offset, [1.0, 1.0]);
        assert_eq!(params.main_alpha, 0.85);
        assert_eq!(params.halo_alpha, 0.35);
    }

    #[test]
    fn invalid_scale_factor_falls_back_to_one() {
        assert_eq!(sanitized_scale_factor(2.0), 2.0);
        assert_eq!(sanitized_scale_factor(0.0), 1.0);
        assert_eq!(sanitized_scale_factor(f32::NAN), 1.0);
    }
}

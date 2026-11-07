//! One final conversion from in-app linear-premultiplied color to OS alpha.
//! Never sample this converted surface for another glass layer.
use wgpu::util::DeviceExt;

pub(super) struct SurfacePresentation {
    pub texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    format: wgpu::TextureFormat,
}

fn alpha_mode_code(mode: wgpu::CompositeAlphaMode) -> u32 {
    match mode {
        wgpu::CompositeAlphaMode::PreMultiplied => 1,
        wgpu::CompositeAlphaMode::PostMultiplied => 2,
        // These modes do not promise a stored-channel alpha interpretation.
        _ => 0,
    }
}

impl SurfacePresentation {
    pub fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OS surface presentation shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_surface_present.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("OS surface presentation pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let values = [
            alpha_mode_code(config.alpha_mode),
            u32::from(config.format.is_srgb()),
            0,
            0,
        ];
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("OS surface presentation uniforms"),
            contents: bytemuck::cast_slice(&values),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let (texture, view, bind_group) = Self::target(
            device,
            config.format,
            config.width,
            config.height,
            &pipeline,
            &uniforms,
        );
        Self {
            texture,
            view,
            bind_group,
            pipeline,
            uniforms,
            format: config.format,
        }
    }

    fn target(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        pipeline: &wgpu::RenderPipeline,
        uniforms: &wgpu::Buffer,
    ) -> (wgpu::Texture, wgpu::TextureView, wgpu::BindGroup) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("completed in-app linear-premultiplied scene"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("OS surface presentation bind group"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniforms.as_entire_binding(),
                },
            ],
        });
        (texture, view, bind_group)
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        (self.texture, self.view, self.bind_group) = Self::target(
            device,
            self.format,
            width,
            height,
            &self.pipeline,
            &self.uniforms,
        );
    }

    pub fn view(&self) -> wgpu::TextureView {
        self.view.clone()
    }

    pub fn present(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        self.present_with_timestamps(encoder, target, None);
    }

    pub fn present_with_timestamps(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("OS surface presentation pass"),
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
            timestamp_writes,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::alpha_mode_code;
    use wgpu::CompositeAlphaMode::*;

    #[test]
    fn explicit_alpha_modes_select_their_stored_channel_contract() {
        assert_eq!(alpha_mode_code(PreMultiplied), 1);
        assert_eq!(alpha_mode_code(PostMultiplied), 2);
        for mode in [Auto, Inherit, Opaque] {
            assert_eq!(alpha_mode_code(mode), 0);
        }
    }
}

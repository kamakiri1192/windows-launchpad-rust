//! Glyph atlas texture + text instance buffer for label glyphs.
//!
//! The atlas texture is allocated in [`Renderer::new`] at the initial atlas
//! size and re-uploaded only when the CPU-side atlas becomes dirty (new
//! glyphs added). When the CPU-side atlas grows, the texture is reallocated
//! at the new size and every bind group that samples it is rebuilt (same
//! pattern as the icon atlas). The per-label glyph quad buffer is rebuilt on
//! a relayout, not on every frame.

use super::Renderer;

impl Renderer {
    /// Upload the glyph atlas texture from the given RGBA buffer.
    ///
    /// Reallocates the texture (and rebuilds the bind groups that sample it)
    /// when `(w, h)` differs from the current texture size, i.e. after the
    /// CPU-side atlas has grown.
    pub fn upload_atlas(&mut self, rgba: &[u8], w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let reallocated = self.atlas_texture.width() != w || self.atlas_texture.height() != h;
        if reallocated {
            self.atlas_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("glyph atlas"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.rebind_text_atlas();
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Rebuild every bind group that samples the glyph atlas against the
    /// current texture. Used after `atlas_texture` is reallocated (growth).
    fn rebind_text_atlas(&mut self) {
        let view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // Grid/modal/etc. glyph pass: uniform + atlas + sampler.
        self.atlas_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bg"),
            layout: &self.text_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        // Control overlay passes share one bind group (control uniforms +
        // atlas + sampler); the shape pipeline binds only [0] but the text
        // pipeline samples the atlas.
        let control_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("control bg"),
            layout: &self.control_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.control_uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.control_bind_group = control_bg.clone();
        self.control_text_bind_group = control_bg;
    }
}

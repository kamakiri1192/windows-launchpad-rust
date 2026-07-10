//! Icon atlas texture + per-icon instance buffer.
//!
//! The atlas starts as a 1×1 placeholder allocated in [`Renderer::new`]; it is
//! replaced once the launcher's icon set is loaded. Subsequent per-icon
//! updates go through [`Renderer::write_icon_cell`], which writes only the
//! changed cell instead of re-blitting the whole texture.

use wgpu::TextureViewDescriptor;

use super::counters::Category;
use crate::renderer::icon_pipeline::IconInstance;

use super::Renderer;

impl Renderer {
    /// Upload the icon atlas, replacing the 1×1 placeholder created in `new`.
    ///
    /// Reallocates the texture to match `(w, h)` and rebuilds the bind group
    /// that points the icon pipeline at it. Call once after icons are loaded;
    /// safe to call again if the atlas changes (e.g. app list refresh).
    pub fn upload_icon_atlas(&mut self, rgba: &[u8], w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let reallocated =
            self.icon_atlas_texture.width() != w || self.icon_atlas_texture.height() != h;
        if reallocated {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("icon atlas"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB-encoded: icon pixels are stored as sRGB bytes, so sampling
                // auto-decodes to linear for correct compositing onto the sRGB
                // surface. Using plain Rgba8Unorm would double-apply gamma and wash
                // colors out.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.icon_atlas_texture = texture;
            self.rebind_icon_atlas();
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.icon_atlas_texture,
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

    /// Update a single icon's pixels in the existing atlas texture (fixed-slot
    /// design). `(x, y)` is the icon's top-left inside the texture (including
    /// the cell padding); `w`/`h` is the icon bitmap size. Cheaper than a full
    /// `upload_icon_atlas` re-blit and leaves all other UVs untouched.
    pub fn write_icon_cell(&self, rgba: &[u8], x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.icon_atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
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

    /// Current icon-atlas texture dimensions, so callers can tell whether a
    /// full re-upload (reallocate) is needed vs. a partial cell write.
    pub fn icon_atlas_size(&self) -> (u32, u32) {
        (
            self.icon_atlas_texture.width(),
            self.icon_atlas_texture.height(),
        )
    }

    /// Rebuild the icon atlas bind group against the current texture. Used
    /// after `icon_atlas_texture` is reallocated.
    fn rebind_icon_atlas(&mut self) {
        let view = self
            .icon_atlas_texture
            .create_view(&TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("icon atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.icon_atlas_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icon atlas bg"),
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
        self.counters.record_atlas_rebind();
    }

    /// Replace the per-icon instance buffer (one entry per tile with an icon).
    pub fn set_icon_instances(&mut self, instances: &[IconInstance]) {
        self.dragged_icon_instance = instances
            .last()
            .map(|i| (i.extra[3] as u32 & 2) != 0)
            .unwrap_or(false);
        let outcome = self
            .icon_instance_buffer
            .set(&self.device, &self.queue, instances);
        if outcome.allocated {
            self.counters.record_creation(Category::Icon);
        }
    }
}

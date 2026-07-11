//! Glyph atlas texture + text instance buffer for label glyphs.
//!
//! The atlas texture is allocated in [`Renderer::new`] and re-uploaded only
//! when the CPU-side atlas becomes dirty (new glyphs added). The per-label
//! glyph quad buffer is rebuilt on a relayout, not on every frame.

use super::Renderer;

impl Renderer {
    /// Upload the glyph atlas texture from the given RGBA buffer.
    pub fn upload_atlas(&self, rgba: &[u8]) {
        let (w, h) = crate::renderer::text_engine::TextRenderer::atlas_dimensions();
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
}

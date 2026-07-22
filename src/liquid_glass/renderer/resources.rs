//! GPU resource factories and persistent shape-buffer capacity policy.

use std::num::NonZeroU64;

use wgpu::util::DeviceExt;

use super::{GlassShape, GlassUniforms, BACKDROP_FORMAT, BLUR_FORMAT, GEOMETRY_FORMAT};

pub(super) fn create_shape_buffer(device: &wgpu::Device, shapes: &[GlassShape]) -> wgpu::Buffer {
    let fallback;
    let slice = if shapes.is_empty() {
        fallback = vec![GlassShape::rounded_rect([0.0, 0.0], [1.0, 1.0], 1.0)];
        fallback.as_slice()
    } else {
        shapes
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("liquid glass shape buffer"),
        contents: bytemuck::cast_slice(slice),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

pub(super) fn create_shape_buffer_with_capacity(
    device: &wgpu::Device,
    capacity: usize,
    label: &'static str,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (std::mem::size_of::<GlassShape>() * capacity.max(1)) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(super) fn next_shape_capacity(current: usize, needed: usize) -> usize {
    needed.max(current.saturating_mul(2)).max(1)
}

pub(super) fn create_uniform_buffer(
    device: &wgpu::Device,
    label: &'static str,
    uniforms: &GlassUniforms,
) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::bytes_of(uniforms),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

pub(super) fn create_geometry_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_texture(
        device,
        "liquid glass geometry texture",
        width,
        height,
        GEOMETRY_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

pub(super) fn create_overlay_geometry_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_texture(
        device,
        "liquid glass overlay geometry texture",
        width,
        height,
        GEOMETRY_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

pub(super) fn create_backdrop_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_texture(
        device,
        "liquid glass backdrop texture",
        width,
        height,
        BACKDROP_FORMAT,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    )
}

pub(super) fn create_gpu_backdrop_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_texture(
        device,
        "liquid glass GPU backdrop texture",
        width,
        height,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    )
}

fn create_texture(
    device: &wgpu::Device,
    label: &'static str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
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
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

pub(super) fn create_blur_texture_raw(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    level: u32,
    label: &'static str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (width, height) = blur_level_extent(width, height, level);
    create_texture(
        device,
        label,
        width,
        height,
        BLUR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    )
}

pub(super) fn blur_level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    let mut width = width.max(1);
    let mut height = height.max(1);
    for _ in 0..level.min(3) {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    (width, height)
}

pub(super) fn upload_initial_backdrop(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) {
    let pixels = vec![0u8; (width.max(1) * height.max(1) * 4) as usize];
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width.max(1) * 4),
            rows_per_image: Some(height.max(1)),
        },
        wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
    );
}

pub(super) fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(std::mem::size_of::<GlassUniforms>() as u64),
        },
        count: None,
    }
}

pub(super) fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(super) fn create_geometry_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    shapes: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass geometry bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: shapes.as_entire_binding(),
            },
        ],
    })
}

pub(super) fn create_final_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    backdrop_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    geometry_view: &wgpu::TextureView,
    blur_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass final bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(backdrop_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(geometry_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(blur_view),
            },
        ],
    })
}

fn create_blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass blur bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

pub(super) fn create_blur_pyramid_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    backdrop_view: &wgpu::TextureView,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    blur_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> ([wgpu::BindGroup; 3], [wgpu::BindGroup; 3]) {
    let down = [
        create_blur_bind_group(device, layout, backdrop_view, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
    ];
    let up = [
        create_blur_bind_group(device, layout, &levels[2].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
    ];
    let _ = blur_view;
    (down, up)
}

#[cfg(test)]
mod tests {
    use super::next_shape_capacity;

    #[test]
    fn shape_capacity_grows_only_past_current_capacity() {
        assert_eq!(next_shape_capacity(1, 2), 2);
        assert_eq!(next_shape_capacity(8, 9), 16);
        assert_eq!(next_shape_capacity(8, 20), 20);
    }
}

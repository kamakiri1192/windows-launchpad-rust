use std::{
    fs,
    path::{Path, PathBuf},
};

#[test]
fn all_wgsl_shaders_compile_with_wgpu_validation() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut shader_paths = Vec::new();
    collect_wgsl_files(&manifest_dir.join("src"), &mut shader_paths);
    shader_paths.sort();

    assert!(
        !shader_paths.is_empty(),
        "expected at least one .wgsl shader under src/"
    );

    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = request_adapter(&instance).await;
        let (device, _queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("wgsl validation device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create wgpu device for WGSL validation");

        for path in shader_paths {
            let source = fs::read_to_string(&path).expect("failed to read WGSL shader");
            let label = path
                .strip_prefix(&manifest_dir)
                .unwrap_or(&path)
                .display()
                .to_string();

            let _shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        }
    });
}

async fn request_adapter(instance: &wgpu::Instance) -> wgpu::Adapter {
    if let Ok(adapter) = instance.request_adapter(&adapter_options(false)).await {
        return adapter;
    }

    instance
        .request_adapter(&adapter_options(true))
        .await
        .expect("failed to request a wgpu adapter for WGSL validation")
}

fn adapter_options(force_fallback_adapter: bool) -> wgpu::RequestAdapterOptions<'static, 'static> {
    wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter,
    }
}

fn collect_wgsl_files(dir: &Path, output: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("failed to read shader directory") {
        let path = entry.expect("failed to read shader directory entry").path();

        if path.is_dir() {
            collect_wgsl_files(&path, output);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("wgsl") {
            output.push(path);
        }
    }
}

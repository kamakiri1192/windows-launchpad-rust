//! Architecture boundary tests for Phase 6.5.
//!
//! These tests verify the production action/command dispatch path and the
//! source-layout boundaries introduced by Phase 6.5:
//!
//! - The handler dispatches through `AppAction` (no inline side effects).
//! - `AppCommand::LaunchApp` exists and is distinct from `HideWindow`
//!   (hide-before-launch ordering).
//! - `AppCommand::HideWithClickPassthrough` is distinct from `HideWindow`
//!   (modal dismiss without click replay).
//! - The domain layer is library-public (compiles without wgpu/winit).
//! - Shader-facing GPU structs (`TileInstance`, `ControlInstance`) live in the
//!   renderer facade, not in feature or bin-adapter modules.
//!
//! These are lightweight: they check type existence, variant distinctness,
//! and module visibility. Behavioral correctness is covered by the unit tests
//! in `src/app/action.rs`.

// The binary crate re-exports its modules via `crate::`, but integration tests
// link against the library crate. We only exercise the library-public surface
// here; the binary-private dispatch is covered by the in-crate unit tests.

#[test]
fn domain_layer_is_library_public() {
    // domain is exposed via lib.rs, so it is reachable from the integration
    // test target. This compiles only if domain has no wgpu/winit deps.
    use launchpad_windows::domain;
    let _ = std::marker::PhantomData::<domain::app_id::AppId>;
    let _ = std::marker::PhantomData::<domain::settings::Settings>;
}

#[test]
fn ui_model_uv_rect_is_library_public() {
    // UvRect was moved from icons/ (bin-only) to ui_model (library) so domain
    // types can reference it without pulling in wgpu/winit.
    use launchpad_windows::ui_model::geometry::UvRect;
    let _ = std::marker::PhantomData::<UvRect>;
}

#[test]
fn layout_and_ui_model_are_library_public() {
    use launchpad_windows::layout;
    use launchpad_windows::ui_model;
    let _ = std::marker::PhantomData::<layout::LayoutResult>;
    let _ = std::marker::PhantomData::<ui_model::render_model::RenderModel>;
}

#[test]
fn glass_layer_is_renderer_neutral() {
    // GlassLayer (Base/Overlay/Modal) is renderer-neutral compositing intent,
    // not a feature-specific pass selector.
    use launchpad_windows::ui_model::render_model::GlassLayer;
    assert_eq!(GlassLayer::Base, GlassLayer::Base);
    assert_eq!(GlassLayer::Overlay, GlassLayer::Overlay);
    assert_eq!(GlassLayer::Modal, GlassLayer::Modal);
    assert_ne!(GlassLayer::Base, GlassLayer::Modal);
}

fn rust_sources(relative: &str) -> String {
    fn visit(path: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(path).expect("read source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                visit(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                out.push_str(&std::fs::read_to_string(path).expect("read Rust source"));
                out.push('\n');
            }
        }
    }

    let mut out = String::new();
    visit(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative),
        &mut out,
    );
    out
}

#[test]
fn forbidden_lower_layer_dependencies_are_absent() {
    let features = rust_sources("src/features");
    assert!(
        !features.contains("crate::renderer"),
        "features -> renderer"
    );
    assert!(
        !features.contains("crate::platform"),
        "features -> platform"
    );

    let layout = rust_sources("src/layout");
    assert!(!layout.contains("crate::renderer"), "layout -> renderer");

    let renderer = rust_sources("src/renderer");
    assert!(
        !renderer.contains("crate::features"),
        "renderer -> features"
    );
    assert!(
        !renderer.contains("crate::grid"),
        "renderer -> binary grid adapter"
    );

    let domain = rust_sources("src/domain");
    for forbidden in ["wgpu::", "winit::", "windows::Win32"] {
        assert!(!domain.contains(forbidden), "domain contains {forbidden}");
    }

    let workers = rust_sources("src/workers");
    assert!(!workers.contains("crate::app::"), "workers -> app");
    assert!(!workers.contains("crate::renderer"), "workers -> renderer");
}

#[test]
fn renderer_does_not_receive_domain_launcher_concepts() {
    // Phase 7: the renderer must not import LauncherItem, Folder, FolderId, or
    // LauncherState. AppId is allowed to remain in the domain (it predates
    // Phase 7 and is referenced by icon/diff types the renderer-adjacent code
    // consumes through ui_model), but the item/folder layout concepts must stay
    // behind the app/layout boundary and cross into the renderer only as
    // renderer-neutral RenderModel primitives.
    let renderer = rust_sources("src/renderer");
    for forbidden in [
        "LauncherItem",
        "LauncherState",
        "domain::folders::Folder",
        "domain::launcher_item",
        "domain::launcher_state",
    ] {
        assert!(
            !renderer.contains(forbidden),
            "renderer imports domain concept: {forbidden}"
        );
    }
}

#[test]
fn domain_launcher_item_and_folder_are_library_public() {
    use launchpad_windows::domain;
    let _ = std::marker::PhantomData::<domain::launcher_item::LauncherItem>;
    let _ = std::marker::PhantomData::<domain::folders::FolderId>;
    let _ = std::marker::PhantomData::<domain::folders::Folder>;
    let _ = std::marker::PhantomData::<domain::launcher_state::LauncherState>;
}

#[test]
fn renderer_scene_submission_is_prepare_only() {
    let renderer = rust_sources("src/renderer");
    for forbidden in [
        "pub fn set_tile_instances",
        "pub fn set_icon_instances",
        "pub fn set_text_instances",
        "pub fn set_control_instances",
        "pub fn set_gear_instances",
        "pub fn set_settings_instances",
        "pub fn set_overlay_glass",
        "pub fn rebuild_instances",
    ] {
        assert!(!renderer.contains(forbidden), "legacy facade: {forbidden}");
    }
    assert!(renderer.contains("pub fn prepare(&mut self, model: &RenderModel)"));
}

#[test]
fn edit_badge_frame_motion_is_gpu_driven() {
    let badges = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/renderer/badges.rs"),
    )
    .expect("badge source");
    assert!(!badges.contains("animated_badge_center"));
    assert!(!badges.contains("fn update_edit_badges"));

    let control_shader = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shader_control.wgsl"),
    )
    .expect("control shader");
    let glass_shader = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/shaders/liquid_glass_geometry.wgsl"),
    )
    .expect("glass shader");
    assert!(control_shader.contains("u.viewport_scroll.w + kind.w"));
    assert!(glass_shader.contains("u.time + shape.motion.z"));
}

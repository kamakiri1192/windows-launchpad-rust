//! Publish the first fresh drawable and its layer visibility together.
//!
//! Metal returns Occluded while the NSWindow is hidden, so rendering before
//! orderFront cannot flush its old drawable. Mask the resident layer on hide
//! and use transactional presentation until its first fresh frame is published.
use objc2_quartz_core::CATransaction;

use super::Renderer;

impl Renderer {
    pub fn native_reveal_pending(&self) -> bool {
        self.native_reveal_pending
    }

    pub fn prepare_native_reveal(&mut self) {
        let Some(surface) = &self.surface else {
            return;
        };
        // SAFETY: retain wgpu ownership; only change main-thread presentation
        // properties under the HAL's layer lock. No Metal resource is destroyed.
        let Some(surface) = (unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }) else {
            return;
        };
        let layer = surface.render_layer().lock();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setOpacity(0.0);
        layer.setPresentsWithTransaction(true);
        CATransaction::commit();
        self.native_reveal_pending = true;
        self.native_reveal_capture_sequence = self.liquid_glass.capture_sequence();
    }

    pub(super) fn present_resident_frame(&mut self, present: impl FnOnce()) {
        if !self.native_reveal_pending {
            present();
            return;
        }
        // Keep alpha=0 until a post-resume capture has actually been consumed.
        // If capture failed, allow the existing fallback material instead of
        // leaving the launcher invisible indefinitely.
        let backdrop_ready = !self
            .liquid_glass
            .needs_fresh_capture(self.native_reveal_capture_sequence);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // wgpu's transactional Metal path waits until the submitted commands
        // are scheduled, then presents the drawable in this CA transaction.
        present();
        if backdrop_ready {
            if let Some(surface) = &self.surface {
                // SAFETY: same lifetime/ownership rules as prepare_native_reveal.
                if let Some(surface) = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() } {
                    let layer = surface.render_layer().lock();
                    layer.setOpacity(1.0);
                    layer.setPresentsWithTransaction(false);
                }
            }
        }
        CATransaction::commit();
        self.native_reveal_pending = !backdrop_ready;
    }
}

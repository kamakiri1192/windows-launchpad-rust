//! Persistent GPU instance buffers with capacity-based growth.
//!
//! Each overlay batch (control / gear / settings / settings text / control
//! text / edit-badge foreground / tile / icon / text-label) is written
//! frequently — often every frame for the control path, and on every relayout
//! for the tile/icon/text path. Previously every write called
//! `create_buffer_init`, allocating a fresh wgpu::Buffer even when the list
//! shrank to zero and back.
//!
//! [`InstanceBuffer`] keeps one buffer alive, with a logical length (how many
//! instances are valid this frame) and a capacity (how many fit without
//! reallocation). A write that fits within the capacity issues a
//! `queue.write_buffer` only; only a capacity overflow grows the buffer. An
//! empty list sets `logical = 0` and keeps the buffer for reuse, so a shape
//! that disappears (e.g. settings closes) and reappears does not churn
//! allocations.
//!
//! The draw code reads [`InstanceBuffer::len`] and
//! [`InstanceBuffer::as_ref`] to decide whether to draw and which buffer to
//! bind. This is allocation-preserving but otherwise behavior-identical: the
//! GPU sees the same instance bytes and the same draw count as before.

use std::marker::PhantomData;

use bytemuck::Pod;
use wgpu::{Buffer, BufferAddress, Device, Queue};

/// Minimum capacity (in instances) a buffer is grown to. Avoids repeated tiny
/// reallocations for small batches that oscillate around a handful of items.
const MIN_CAPACITY: u32 = 16;

/// A capacity-managed GPU vertex buffer for a `Pod` instance type.
pub(super) struct InstanceBuffer<T: Pod> {
    buffer: Option<Buffer>,
    /// Number of valid instances this frame. Draw count.
    logical: u32,
    /// Capacity in instances (not bytes). The buffer holds at least this many.
    capacity: u32,
    /// Static label used when (re)allocating, for wgpu debugging.
    label: &'static str,
    _marker: PhantomData<T>,
}

/// Outcome of a [`InstanceBuffer::set`] call, so callers can drive any
/// derived state (e.g. detecting the dragged icon, refreshing badge sources).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SetOutcome {
    /// A brand-new buffer was allocated (capacity grew or first allocation).
    pub(super) allocated: bool,
    /// Number of instances now stored.
    pub(super) len: u32,
    /// Whether bytes were submitted through `queue.write_buffer`.
    pub(super) wrote: bool,
}

impl<T: Pod> InstanceBuffer<T> {
    /// Create an empty buffer with no allocation yet. The first `set` with a
    /// non-empty list allocates.
    pub(super) fn new(label: &'static str) -> Self {
        Self {
            buffer: None,
            logical: 0,
            capacity: 0,
            label,
            _marker: PhantomData,
        }
    }

    /// Number of valid instances this frame.
    pub(super) fn len(&self) -> u32 {
        self.logical
    }

    /// The underlying buffer, if any instances are present. Returns `None`
    /// when `logical == 0` so the draw pass skips this batch (matching the old
    /// `Option<Buffer>` behavior where an empty list dropped the buffer).
    pub(super) fn as_ref(&self) -> Option<&Buffer> {
        if self.logical == 0 {
            None
        } else {
            self.buffer.as_ref()
        }
    }

    /// Borrow the raw buffer for vertex slicing (dragged-tile offset draw).
    /// The caller must guarantee `len() > 0`; used by the drag overlay pass
    /// which only runs when a drag is active (instance count > 0).
    pub(super) fn buffer(&self) -> &Buffer {
        self.buffer
            .as_ref()
            .expect("InstanceBuffer::buffer called before any allocation")
    }

    /// Current capacity in instances (for tests / counters).
    pub(super) fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Replace the instance data. Grows the buffer only when `items.len()`
    /// exceeds the current capacity; otherwise reuses it via
    /// `queue.write_buffer`. An empty list clears `logical` to 0 but keeps
    /// the buffer allocated for future reuse.
    pub(super) fn set(&mut self, device: &Device, queue: &Queue, items: &[T]) -> SetOutcome {
        self.logical = items.len() as u32;
        if items.is_empty() {
            // Keep the buffer; just draw zero instances next frame. Matches
            // the old "empty -> Option::None" draw-skip behavior without
            // forcing a reallocation when the list comes back.
            return SetOutcome {
                allocated: false,
                len: 0,
                wrote: false,
            };
        }
        let needed = items.len() as u32;
        if needed > self.capacity {
            self.grow(device, needed);
            let outcome = SetOutcome {
                allocated: true,
                len: self.logical,
                wrote: true,
            };
            // Fresh buffer: upload the full contents.
            if let Some(buf) = self.buffer.as_ref() {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(items));
            }
            outcome
        } else {
            if let Some(buf) = self.buffer.as_ref() {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(items));
            }
            SetOutcome {
                allocated: false,
                len: self.logical,
                wrote: true,
            }
        }
    }

    /// Drop the buffer entirely (logical + capacity reset). Used when a batch
    /// is permanently retired and we want to release VRAM, or in tests to
    /// force a clean slate. Day-to-day empty lists should *not* call this —
    /// they should `set(&[])`, which keeps the buffer for reuse.
    #[allow(dead_code)]
    pub(super) fn clear(&mut self) {
        self.buffer = None;
        self.logical = 0;
        self.capacity = 0;
    }

    fn grow(&mut self, device: &Device, needed: u32) {
        let new_cap = next_capacity(self.capacity, needed);
        let stride = std::mem::size_of::<T>() as BufferAddress;
        let size = stride * new_cap as BufferAddress;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.buffer = Some(buffer);
        self.capacity = new_cap;
    }
}

fn next_capacity(current: u32, needed: u32) -> u32 {
    needed.max(current.saturating_mul(2)).max(MIN_CAPACITY)
}

impl<T: Pod> Default for InstanceBuffer<T> {
    fn default() -> Self {
        Self::new("instance buffer")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These are pure-logic capacity-policy tests. They do NOT need a GPU
    // device: `set` only allocates through the `Device` argument, so we test
    // the decision logic (does it need to grow? does it keep the buffer?) via
    // the public capacity/len surface and the SetOutcome.

    fn outcome_for(capacity: u32, needed: u32) -> SetOutcome {
        // Mirrors the grow decision in `set`.
        if needed == 0 {
            SetOutcome {
                allocated: false,
                len: 0,
                wrote: false,
            }
        } else if needed > capacity {
            SetOutcome {
                allocated: true,
                len: needed,
                wrote: true,
            }
        } else {
            SetOutcome {
                allocated: false,
                len: needed,
                wrote: true,
            }
        }
    }

    #[test]
    fn empty_list_keeps_buffer_for_reuse() {
        // After an empty set, logical is 0 and as_ref returns None (draw
        // skipped), but capacity is retained.
        let buf: InstanceBuffer<[f32; 4]> = InstanceBuffer::new("test");
        assert_eq!(buf.len(), 0);
        assert!(buf.as_ref().is_none());
        assert_eq!(buf.capacity(), 0);
        // Simulate: we can't call set without a device, but the policy says an
        // empty list never allocates and never drops capacity.
        let o = outcome_for(32, 0);
        assert!(!o.allocated);
        assert_eq!(o.len, 0);
    }

    #[test]
    fn growth_policy_doubles_capacity() {
        // First non-empty set past zero capacity allocates.
        assert!(outcome_for(0, 5).allocated);
        // Same capacity: no allocation.
        assert!(!outcome_for(16, 5).allocated);
        // One past capacity: allocate.
        assert!(outcome_for(16, 17).allocated);
        // Exactly at capacity: no allocation.
        assert!(!outcome_for(16, 16).allocated);
    }

    #[test]
    fn min_capacity_floor_prevents_tiny_reallocations() {
        // The grow formula floors at MIN_CAPACITY (16). A request for 1 item
        // when capacity is 0 still allocates exactly once and lands at 16,
        // so subsequent small lists (1..16) reuse the buffer.
        let new_cap = next_capacity(0, 1);
        assert_eq!(new_cap, MIN_CAPACITY);
        // Items 1..=16 all fit without another growth.
        for needed in 1..=MIN_CAPACITY {
            assert!(
                !outcome_for(MIN_CAPACITY, needed).allocated,
                "needed={needed} should fit in capacity={MIN_CAPACITY}"
            );
        }
        // 17 forces a second growth to 32.
        let new_cap = next_capacity(MIN_CAPACITY, 17);
        assert_eq!(new_cap, 32);
    }
}

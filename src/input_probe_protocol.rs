//! JSONL protocol shared by the native input probe and scenario runner.

use serde::{Deserialize, Serialize};

pub const INPUT_ROUTING_QA_ENV: &str = "LAUNCHPAD_INPUT_ROUTING_QA";
pub const QA_WHEEL_RECEIVER_ACTIVATION_ENV: &str = "LAUNCHPAD_QA_WHEEL_RECEIVER_ACTIVATION";
pub const QA_PASSIVE_MACOS_PROBE_ENV: &str = "LAUNCHPAD_QA_PASSIVE_MACOS_PROBE";
pub const MACOS_PRODUCT_EVENT_TAG: i64 = 0x4c50_5f49_504f; // "LP_IPO"

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum ProbeRecord {
    Ready {
        pid: u32,
        top_level: u64,
        child: u64,
        rect: NativeRect,
    },
    Input {
        serial: u64,
        timestamp: u64,
        event: ProbeEvent,
        target: u64,
        root: u64,
        pid: u32,
        screen: NativePoint,
        local: NativePoint,
        foreground: u64,
    },
    LauncherSnapshot {
        serial: u64,
        pid: u32,
        window: u64,
        visible: bool,
        focused: bool,
        z_order: i64,
        generation: u64,
        region: String,
        router_state: String,
        page_position: f32,
        pointer_x: f32,
        pointer_y: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativePoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct NativeRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeEvent {
    MouseMove,
    ButtonDown {
        button: ProbeButton,
    },
    ButtonUp {
        button: ProbeButton,
    },
    /// Native context-menu request produced by normal right-click processing.
    ContextMenu,
    VerticalWheel {
        /// Legacy integral wheel delta (`WHEEL_DELTA` units on Windows).
        delta: i32,
        /// Native horizontal/vertical deltas without quantization.
        delta_x: f64,
        delta_y: f64,
        precise: bool,
        key_state: u16,
        phase: NativePhase,
        momentum_phase: NativePhase,
    },
    FocusGained,
    FocusLost,
    Activated {
        active: bool,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativePhase {
    Began,
    Changed,
    Ended,
    Cancelled,
    MomentumBegan,
    MomentumChanged,
    MomentumEnded,
    Unavailable,
}

impl ProbeRecord {
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json_line(line: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_round_trip_preserves_signed_delta_and_negative_coordinates() {
        let record = ProbeRecord::Input {
            serial: 3,
            timestamp: 99,
            event: ProbeEvent::VerticalWheel {
                delta: -30,
                delta_x: 0.0,
                delta_y: -30.0,
                precise: true,
                key_state: 0x000c,
                phase: NativePhase::Unavailable,
                momentum_phase: NativePhase::Unavailable,
            },
            target: 0x1234,
            root: 0x1000,
            pid: 42,
            screen: NativePoint { x: -1800, y: 20 },
            local: NativePoint { x: 40, y: 80 },
            foreground: 0x1000,
        };
        let line = record.to_json_line().unwrap();
        assert!(!line.contains('\n'));
        assert_eq!(ProbeRecord::from_json_line(&line).unwrap(), record);
    }
}

//! Wire protocol: JSON messages, phone -> server only (SPEC §2.4).

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Absolute,
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Btn {
    Left,
    Right,
}

/// Calibration corner identifiers (SPEC §4.3 / §5.1). Only the 4 real corners;
/// the spec's "Center" point is intentionally unused, so it isn't modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

/// Every message the phone can send. Tagged by the `t` field.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Switch between absolute / relative.
    Mode { mode: Mode },
    /// Store current orientation for a screen corner.
    Calib {
        point: Corner,
        beta: f64,
        alpha: f64,
        #[serde(default)]
        gamma: f64,
    },
    /// Clear all calibration points.
    ResetCalib,
    /// Main orientation stream (~60 Hz).
    Move {
        beta: f64,
        alpha: f64,
        #[serde(default)]
        gamma: f64,
    },
    /// Press a mouse button (hold).
    Down { button: Btn },
    /// Release a mouse button.
    Up { button: Btn },
    /// Wheel scroll; `dy` is a small signed tick count.
    Scroll { dy: f64 },
    /// Filter response (0..1]; the UI exposes the inverse as smoothing amount.
    Smoothing { value: f64 },
}

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

/// Calibration corner identifiers (SPEC §4.3 / §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
    Center,
}

/// Every message the phone can send. Tagged by the `t` field.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Switch between absolute / relative.
    Mode { mode: Mode },
    /// Store current orientation for a screen corner.
    Calib { point: Corner, beta: f64, alpha: f64 },
    /// Clear all calibration points.
    ResetCalib,
    /// Main orientation stream (~60 Hz).
    Move {
        beta: f64,
        alpha: f64,
        #[serde(default)]
        gamma: f64,
        /// accelerationIncludingGravity [x, y, z], used only for anti-jitter.
        #[serde(default)]
        accel: Option<[f64; 3]>,
    },
    /// Press a mouse button (hold).
    Down { button: Btn },
    /// Release a mouse button.
    Up { button: Btn },
    /// Wheel scroll; `dy` is a small signed tick count.
    Scroll { dy: f64 },
    /// Toggle dynamic anti-jitter.
    AntiJitter { on: bool },
    /// Smoothing slider value (0..1].
    Smoothing { value: f64 },
}

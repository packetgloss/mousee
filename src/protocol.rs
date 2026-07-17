//! Wire protocol: JSON messages, phone -> server only (SPEC §2.4).

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RotationDelta {
    /// Unrolled horizontal angular displacement accumulated by the client.
    pub alpha: f64,
    /// Unrolled vertical angular displacement accumulated by the client.
    pub beta: f64,
    /// Accumulated phone roll (diagnostics only).
    pub gamma: f64,
    /// Raw device-local Z/X/Y displacement, retained for diagnostics.
    #[serde(default)]
    pub local_alpha: f64,
    #[serde(default)]
    pub local_beta: f64,
    #[serde(default)]
    pub local_gamma: f64,
    /// Total sensor time represented by this frame.
    #[serde(default)]
    pub sample_ms: f64,
}

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
        /// Client-integrated and orientation-corrected gyroscope displacement.
        #[serde(default)]
        rotation_delta: Option<RotationDelta>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_accepts_integrated_rotation_delta() {
        let message: ClientMsg = serde_json::from_str(
            r#"{"t":"move","alpha":1,"beta":2,"gamma":3,"rotation_delta":{"alpha":10,"beta":20,"gamma":30,"local_alpha":4,"local_beta":5,"local_gamma":6,"sample_ms":16}}"#,
        )
        .unwrap();
        match message {
            ClientMsg::Move {
                rotation_delta: Some(delta),
                ..
            } => {
                assert_eq!((delta.alpha, delta.beta, delta.gamma), (10.0, 20.0, 30.0));
                assert_eq!((delta.local_alpha, delta.local_beta), (4.0, 5.0));
                assert_eq!(delta.sample_ms, 16.0);
            }
            _ => panic!("rotation delta was not parsed"),
        }
    }

    #[test]
    fn move_keeps_rotation_delta_optional_for_fallback_clients() {
        let message: ClientMsg =
            serde_json::from_str(r#"{"t":"move","alpha":1,"beta":2,"gamma":3}"#).unwrap();
        assert!(matches!(
            message,
            ClientMsg::Move {
                rotation_delta: None,
                ..
            }
        ));
    }
}

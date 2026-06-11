//! All tunable parameters live here as explicit constants (SPEC §8).

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// Default TCP port that serves BOTH the HTML page and the WebSocket (SPEC §2.1).
pub const DEFAULT_PORT: u16 = 8081;

/// Hard override for LAN-IP autodetection. `Some("192.168.1.50")` to force it.
pub const PREFERRED_IP: Option<&str> = None;

/// Directory (relative to CWD) where the cached self-signed cert/key/ip live.
pub const CERT_DIR: &str = "mousee-cert";

// ---------------------------------------------------------------------------
// Scrolling (SPEC §7)
// ---------------------------------------------------------------------------

/// Multiplier applied to each scroll tick coming from the phone.
pub const SCROLL_SENSITIVITY: i32 = 1;
/// Direction of the wheel. Flip to invert scroll (swipe-up vs feed direction).
pub const SCROLL_SIGN: i32 = -1;

// ---------------------------------------------------------------------------
// Absolute mode smoothing / anti-jitter (SPEC §5.1)
// ---------------------------------------------------------------------------

/// Smoothing factor used when the phone has not sent a slider value yet.
/// Lower = lazier/smoother, higher = snappier. Range (0..1].
pub const DEFAULT_SMOOTHING: f64 = 0.35;

/// Anti-jitter: |accel_magnitude - 9.8| above this (m/s²) is considered shaking.
pub const ANTIJITTER_THRESHOLD: f64 = 2.5;
/// Smoothing factor forced for a frame detected as "shaking" (stronger smoothing).
pub const ANTIJITTER_SMOOTHING: f64 = 0.08;

// ---------------------------------------------------------------------------
// Relative / air-mouse mode (SPEC §5.2)
// ---------------------------------------------------------------------------

/// Pixels per degree of orientation change.
pub const REL_SENSITIVITY: f64 = 18.0;
/// Acceleration exponent (>= 1). 1.0 = linear, higher = more "mouse acceleration".
pub const REL_EXP: f64 = 1.3;
/// Per-frame dead zone in degrees; movement below this is ignored (kills drift).
pub const REL_DEADZONE: f64 = 0.35;
/// Low-pass factor for the velocity EMA (0..1], lower = smoother.
pub const REL_VEL_SMOOTH: f64 = 0.55;
/// Axis sign for horizontal (alpha). Flip to mirror left/right.
pub const REL_SIGN_X: f64 = 1.0;
/// Axis sign for vertical (beta). Usually inverted so "aim lower => cursor lower".
pub const REL_SIGN_Y: f64 = -1.0;

// ---------------------------------------------------------------------------
// Fallback geometry (SPEC §2.5, §6)
// ---------------------------------------------------------------------------

pub const FALLBACK_WIDTH: i32 = 1920;
pub const FALLBACK_HEIGHT: i32 = 1080;

// ---------------------------------------------------------------------------
// Logging (SPEC §9)
// ---------------------------------------------------------------------------

/// Minimum interval between throttled log lines, per category (milliseconds).
pub const LOG_THROTTLE_MS: u64 = 250;

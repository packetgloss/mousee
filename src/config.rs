//! All tunable parameters live here as explicit constants (SPEC §8).

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// Default TCP port that serves BOTH the HTML page and the WebSocket (SPEC §2.1).
pub const DEFAULT_PORT: u16 = 8081;

/// Hard override for LAN-IP autodetection. `Some("192.168.1.50")` to force it.
pub const PREFERRED_IP: Option<&str> = None;

/// Sub-folder name (under %LOCALAPPDATA% on Windows, or the OS data dir
/// elsewhere) where the cached local CA and server cert/key live. See `tls::dir`.
pub const CERT_DIR: &str = "mousee";

// ---------------------------------------------------------------------------
// Scrolling (SPEC §7)
// ---------------------------------------------------------------------------

/// Multiplier applied to each scroll tick coming from the phone.
pub const SCROLL_SENSITIVITY: i32 = 1;
/// Direction of the wheel. Flip to invert scroll (swipe-up vs feed direction).
pub const SCROLL_SIGN: i32 = -1;

// ---------------------------------------------------------------------------
// Always-on smoothing (SPEC §5.1)
// ---------------------------------------------------------------------------

/// Smoothing factor used when the phone has not sent a slider value yet.
/// Lower = lazier/smoother, higher = snappier. Range (0..1].
pub const DEFAULT_SMOOTHING: f64 = 0.35;

// ---------------------------------------------------------------------------
// Relative / air-mouse mode (SPEC §5.2)
// ---------------------------------------------------------------------------

/// Base pixels per degree of orientation change, horizontal (yaw/alpha).
/// Usually higher than Y: horizontal aiming uses the whole arm and feels
/// "heavier", so a circular wrist motion otherwise comes out as a vertical oval.
pub const REL_SENSITIVITY_X: f64 = 35.0;
/// Base pixels per degree of orientation change, vertical (pitch/beta).
pub const REL_SENSITIVITY_Y: f64 = 29.0;
/// Velocity-proportional acceleration gain (per deg/frame). 0 = pure linear.
/// Adds extra travel the *faster* you turn, so a quick flick crosses the whole
/// desktop without big arm motion, while slow motion stays linear and precise
/// (unlike a `powf` curve, this does NOT suppress small/slow movements).
pub const REL_ACCEL: f64 = 0.4;
/// Per-frame dead zone in degrees; movement below this is ignored (kills drift).
/// Keep small so slow circular motion is not chewed up.
pub const REL_DEADZONE: f64 = 0.08;
/// Ignore accumulated orientation changes after the browser/network has paused
/// this long. Re-priming is preferable to a resume jump across the desktop.
pub const TRACKING_GAP_MS: u64 = 250;
/// Scale of the radial asinh compressor. Small movement stays nearly linear;
/// large movement is compressed but remains unbounded, preserving fast flicks.
pub const REL_COMPRESSION_SCALE_PX: f64 = 30.0;
/// Frozen gamma coupling is clamped to keep a bad calibration recoverable.
pub const GAMMA_CALIB_MAX_COUPLING: f64 = 2.0;

/// 1€ adaptive low-pass parameters. At slow movement the filter matches the
/// existing smoothing slider; speed raises the cutoff to avoid added latency.
pub const ONE_EURO_BETA: f64 = 0.02;
pub const ONE_EURO_DERIVATIVE_CUTOFF_HZ: f64 = 1.0;
pub const FILTER_REFERENCE_HZ: f64 = 60.0;
/// Axis sign for horizontal (alpha). Flip to mirror left/right.
pub const REL_SIGN_X: f64 = -1.0;
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

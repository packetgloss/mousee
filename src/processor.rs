//! Orientation -> cursor mapping. Holds per-connection state and implements both
//! absolute (calibrated) and relative (air-mouse) modes (SPEC §5, §6).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config;
use crate::monitors::LayoutHandle;
use crate::mouse::MouseCmd;
use crate::protocol::{ClientMsg, Corner, Mode, RotationDelta};

/// Computed calibration bounds derived from the 4 corners (SPEC §5.1).
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_beta: f64,   // top
    max_beta: f64,   // bottom
    alpha_left: f64, // unwrapped continuous axis
    alpha_right: f64,
}

#[derive(Debug, Default)]
struct LowPass {
    value: Option<f64>,
}

impl LowPass {
    fn filter(&mut self, input: f64, alpha: f64) -> f64 {
        let output = self
            .value
            .map_or(input, |previous| previous + alpha * (input - previous));
        self.value = Some(output);
        output
    }

    fn reset(&mut self) {
        self.value = None;
    }
}

#[derive(Debug, Default)]
struct OneEuroAxis {
    signal: LowPass,
    derivative: LowPass,
    previous_raw: Option<f64>,
    integrated_input: f64,
    previous_output: f64,
}

impl OneEuroAxis {
    /// Filter an input delta by integrating it to a trajectory, applying the
    /// standard 1€ position filter, then differentiating the filtered result.
    /// This makes the adaptive derivative represent movement speed.
    fn filter_delta(&mut self, delta: f64, dt: Duration, min_cutoff_hz: f64) -> f64 {
        let seconds = dt.as_secs_f64().max(1.0 / 1000.0);
        self.integrated_input += delta;
        let raw_derivative = self
            .previous_raw
            .map_or(0.0, |previous| (self.integrated_input - previous) / seconds);
        self.previous_raw = Some(self.integrated_input);

        let derivative = self.derivative.filter(
            raw_derivative,
            low_pass_alpha(config::ONE_EURO_DERIVATIVE_CUTOFF_HZ, seconds),
        );
        let cutoff = min_cutoff_hz + config::ONE_EURO_BETA * derivative.abs();
        // Relative velocity starts at rest. Seeding the signal with zero makes
        // the first response match the old EMA instead of passing it raw.
        if self.signal.value.is_none() {
            self.signal.value = Some(0.0);
        }
        let filtered_position = self
            .signal
            .filter(self.integrated_input, low_pass_alpha(cutoff, seconds));
        let output_delta = filtered_position - self.previous_output;
        self.previous_output = filtered_position;
        output_delta
    }

    fn reset(&mut self) {
        self.signal.reset();
        self.derivative.reset();
        self.previous_raw = None;
        self.integrated_input = 0.0;
        self.previous_output = 0.0;
    }
}

#[derive(Debug, Default)]
struct OneEuro2D {
    x: OneEuroAxis,
    y: OneEuroAxis,
}

impl OneEuro2D {
    fn filter(&mut self, input: (f64, f64), dt: Duration, min_cutoff_hz: f64) -> (f64, f64) {
        (
            self.x.filter_delta(input.0, dt, min_cutoff_hz),
            self.y.filter_delta(input.1, dt, min_cutoff_hz),
        )
    }

    fn reset(&mut self) {
        self.x.reset();
        self.y.reset();
    }
}

pub struct Processor {
    layout: LayoutHandle,
    debug: bool,

    mode: Mode,
    smoothing: f64,

    // calibration
    calib: HashMap<Corner, (f64, f64)>, // corner -> (beta, alpha)
    bounds: Option<Bounds>,

    // shared smoothed cursor position (both modes write here)
    pos: (f64, f64),
    has_pos: bool,

    // relative mode state
    prev_alpha: Option<f64>,
    prev_beta: Option<f64>,
    prev_gamma: Option<f64>,
    last_sample: Option<Instant>,
    delta_filter: OneEuro2D,
    vel: (f64, f64),

    // throttled logging
    last_log: HashMap<&'static str, Instant>,
}

impl Processor {
    pub fn new(layout: LayoutHandle, debug: bool) -> Self {
        Self {
            layout,
            debug,
            mode: Mode::Relative, // air-mouse is the recommended default (SPEC §4.2)
            smoothing: config::DEFAULT_SMOOTHING,
            calib: HashMap::new(),
            bounds: None,
            pos: (0.0, 0.0),
            has_pos: false,
            vel: (0.0, 0.0),
            prev_alpha: None,
            prev_beta: None,
            prev_gamma: None,
            last_sample: None,
            delta_filter: OneEuro2D::default(),
            last_log: HashMap::new(),
        }
    }

    /// Reset the per-frame deltas/position so a reconnect or mode switch does
    /// not produce a huge first-frame jump (SPEC §5.2, §10.5).
    fn reset_tracking(&mut self) {
        self.prev_alpha = None;
        self.prev_beta = None;
        self.prev_gamma = None;
        self.last_sample = None;
        self.delta_filter.reset();
        self.vel = (0.0, 0.0);
        self.has_pos = false;
    }

    fn should_log(&mut self, cat: &'static str) -> bool {
        if !self.debug {
            return false;
        }
        let now = Instant::now();
        let due = self
            .last_log
            .get(cat)
            .map(|t| now.duration_since(*t) >= Duration::from_millis(config::LOG_THROTTLE_MS))
            .unwrap_or(true);
        if due {
            self.last_log.insert(cat, now);
        }
        due
    }

    /// Handle one incoming message; returns mouse commands to enqueue.
    pub fn handle(&mut self, msg: ClientMsg) -> Vec<MouseCmd> {
        match msg {
            ClientMsg::Mode { mode } => {
                self.mode = mode;
                self.reset_tracking();
                tracing::info!("mode -> {mode:?}");
                vec![]
            }
            ClientMsg::Smoothing { value } => {
                self.smoothing = value.clamp(0.05, 0.95);
                vec![]
            }
            ClientMsg::Calib {
                point,
                beta,
                alpha,
                gamma,
            } => {
                self.calib.insert(point, (beta, alpha));
                tracing::info!("calib {point:?}: alpha={alpha:.1} beta={beta:.1} gamma={gamma:.1}");
                self.recompute_bounds();
                vec![]
            }
            ClientMsg::ResetCalib => {
                self.calib.clear();
                self.bounds = None;
                tracing::info!("calibration reset");
                vec![]
            }
            ClientMsg::Down { button } => vec![MouseCmd::Press(button)],
            ClientMsg::Up { button } => vec![MouseCmd::Release(button)],
            ClientMsg::Scroll { dy } => {
                let ticks = (dy.round() as i32) * config::SCROLL_SENSITIVITY * config::SCROLL_SIGN;
                if ticks == 0 {
                    vec![]
                } else {
                    vec![MouseCmd::Scroll(ticks)]
                }
            }
            ClientMsg::Move {
                beta,
                alpha,
                gamma,
                rotation_delta,
            } => self.on_move(beta, alpha, gamma, rotation_delta),
        }
    }

    fn recompute_bounds(&mut self) {
        let (Some(tl), Some(tr), Some(bl), Some(br)) = (
            self.calib.get(&Corner::Tl).copied(),
            self.calib.get(&Corner::Tr).copied(),
            self.calib.get(&Corner::Bl).copied(),
            self.calib.get(&Corner::Br).copied(),
        ) else {
            self.bounds = None;
            return;
        };

        let min_beta = (tl.0 + tr.0) / 2.0;
        let max_beta = (bl.0 + br.0) / 2.0;
        let alpha_left = (tl.1 + bl.1) / 2.0;
        let mut alpha_right = (tr.1 + br.1) / 2.0;

        // Unwrap the right edge onto a continuous axis around the left edge
        // (alpha wraps at 0/360) — SPEC §5.1.
        if alpha_right - alpha_left > 180.0 {
            alpha_right -= 360.0;
        } else if alpha_right - alpha_left < -180.0 {
            alpha_right += 360.0;
        }

        self.bounds = Some(Bounds {
            min_beta,
            max_beta,
            alpha_left,
            alpha_right,
        });
        tracing::info!(
            "calibration complete: top={min_beta:.1} bottom={max_beta:.1} left={alpha_left:.1} right={alpha_right:.1}"
        );
    }

    fn on_move(
        &mut self,
        beta: f64,
        alpha: f64,
        gamma: f64,
        rotation_delta: Option<RotationDelta>,
    ) -> Vec<MouseCmd> {
        if self.should_log("sensor") {
            tracing::debug!("sensor: alpha={alpha:.2} beta={beta:.2} gamma={gamma:.2}");
        }
        match self.mode {
            Mode::Absolute => self.absolute(beta, alpha),
            Mode::Relative => self.relative(beta, alpha, gamma, rotation_delta),
        }
    }

    // --- Absolute (calibrated) mode (SPEC §5.1 + §6) -----------------------
    fn absolute(&mut self, beta: f64, alpha: f64) -> Vec<MouseCmd> {
        let Some(b) = self.bounds else {
            return vec![]; // not calibrated yet
        };
        // Snapshot the live layout for this frame (it may change under hotplug).
        let layout = self.layout.current();

        // Pull live alpha onto the continuous axis around the calibrated center.
        let center = (b.alpha_left + b.alpha_right) / 2.0;
        let mut a = alpha;
        while a - center > 180.0 {
            a -= 360.0;
        }
        while a - center < -180.0 {
            a += 360.0;
        }

        let span_x = b.alpha_right - b.alpha_left;
        let span_y = b.max_beta - b.min_beta;
        if span_x.abs() < 1e-6 || span_y.abs() < 1e-6 {
            return vec![];
        }
        let frac_x = (a - b.alpha_left) / span_x;
        let frac_y = (beta - b.min_beta) / span_y;

        // Horizontal spans the whole virtual desktop so the pointer crosses all
        // monitors (SPEC §5.1 / §6 step 1).
        let target_x_f = layout.origin_x as f64 + frac_x * layout.width as f64;
        let target_x = target_x_f.round() as i32;

        // Vertical is stretched over the REAL monitor under the pointer (§6).
        let mon = *layout.monitor_for_x(target_x);
        let cx = target_x.clamp(mon.x, mon.x + mon.w - 1);
        let cy_f = mon.y as f64 + frac_y * mon.h as f64;
        let cy = (cy_f.round() as i32).clamp(mon.y, mon.y + mon.h - 1);
        let target = (cx as f64, cy as f64);

        // Always-on EMA smoothing. The phone slider exposes the inverse amount,
        // while this internal factor remains response: lower is smoother.
        let sf = self.smoothing;

        if !self.has_pos {
            self.pos = target;
            self.has_pos = true;
        } else {
            self.pos.0 = target.0 * sf + self.pos.0 * (1.0 - sf);
            self.pos.1 = target.1 * sf + self.pos.1 * (1.0 - sf);
        }

        let out = (self.pos.0.round() as i32, self.pos.1.round() as i32);

        if self.should_log("abs") {
            tracing::debug!(
                "abs: alpha={alpha:.1}->{a:.1} L={:.1} R={:.1} fx={frac_x:.2} fy={frac_y:.2} mon=({},{}) pos=({},{})",
                b.alpha_left, b.alpha_right, mon.x, mon.y, out.0, out.1
            );
        }

        vec![MouseCmd::MoveTo(out.0, out.1)]
    }

    // --- Relative (air-mouse) mode (SPEC §5.2) -----------------------------
    fn relative(
        &mut self,
        beta: f64,
        alpha: f64,
        gamma: f64,
        rotation_delta: Option<RotationDelta>,
    ) -> Vec<MouseCmd> {
        let now = Instant::now();
        // Need two samples to take a delta; first frame only primes the state.
        let (Some(prev_alpha), Some(prev_beta), Some(prev_gamma), Some(prev_at)) = (
            self.prev_alpha,
            self.prev_beta,
            self.prev_gamma,
            self.last_sample,
        ) else {
            self.prev_alpha = Some(alpha);
            self.prev_beta = Some(beta);
            self.prev_gamma = Some(gamma);
            self.last_sample = Some(now);
            return vec![];
        };

        let dt = now.duration_since(prev_at);
        self.prev_alpha = Some(alpha);
        self.prev_beta = Some(beta);
        self.prev_gamma = Some(gamma);
        self.last_sample = Some(now);

        // A locked/backgrounded browser resumes with a large accumulated
        // orientation delta. Treat the first resumed sample as a new baseline.
        if dt >= Duration::from_millis(config::TRACKING_GAP_MS) {
            self.vel = (0.0, 0.0);
            self.delta_filter.reset();
            if self.should_log("gap") {
                tracing::debug!(
                    "sensor stream resumed after {:.0} ms; tracking re-primed",
                    dt.as_secs_f64() * 1000.0
                );
            }
            return vec![];
        }

        let euler = (
            wrapped_delta(alpha, prev_alpha),
            beta - prev_beta,
            wrapped_delta(gamma, prev_gamma),
        );
        // The browser integrates each DeviceMotion sample against its own
        // timestamp and unrolls the phone-local axes before sending. Older
        // clients (or denied motion permission) retain the Euler fallback.
        let (input, source) = rotation_delta.map_or((euler, "euler"), |delta| {
            ((delta.alpha, delta.beta, delta.gamma), "gyro_unrolled")
        });
        let (da, db, dg) = input;
        let shaped = (
            config::REL_SIGN_X * shape(da, config::REL_SENSITIVITY_X),
            config::REL_SIGN_Y * shape(db, config::REL_SENSITIVITY_Y),
        );
        let compressed = soft_compress_vector(shaped, config::REL_COMPRESSION_SCALE_PX);

        // 1€ filtering: the slider still defines the familiar slow-movement
        // response, while fast deliberate input raises the cutoff automatically.
        let s = self.smoothing;
        let min_cutoff_hz = smoothing_to_cutoff(s);
        self.vel = self.delta_filter.filter(compressed, dt, min_cutoff_hz);

        let layout = self.layout.current();
        let (ox, oy, w, h) = (
            layout.origin_x,
            layout.origin_y,
            layout.width,
            layout.height,
        );
        if !self.has_pos {
            // Preserve the original relative-mode baseline and feel.
            self.pos = (ox as f64 + w as f64 / 2.0, oy as f64 + h as f64 / 2.0);
            self.has_pos = true;
        }

        let min_x = ox as f64;
        let max_x = (ox + w - 1) as f64;
        let min_y = oy as f64;
        let max_y = (oy + h - 1) as f64;
        self.pos.0 = (self.pos.0 + self.vel.0).clamp(min_x, max_x);
        self.pos.1 = (self.pos.1 + self.vel.1).clamp(min_y, max_y);

        let out = (self.pos.0.round() as i32, self.pos.1.round() as i32);

        if self.should_log("rel") {
            tracing::debug!(
                "rel: source={source} gyro_local_deg={} gyro_unrolled_deg={} sensor_ms={:.1} euler_deg=({:.2},{:.2},{:.2}) input_deg=({da:.2},{db:.2},{dg:.2}) shaped=({:.1},{:.1}) compressed=({:.1},{:.1}) cutoff_min={min_cutoff_hz:.1}Hz v=({:.1},{:.1}) pos=({},{})",
                rotation_delta.map_or_else(
                    || "none".to_string(),
                    |d| format!("({:.2},{:.2},{:.2})", d.local_alpha, d.local_beta, d.local_gamma)
                ),
                rotation_delta.map_or_else(
                    || "none".to_string(),
                    |d| format!("({:.2},{:.2},{:.2})", d.alpha, d.beta, d.gamma)
                ),
                rotation_delta.map_or(0.0, |d| d.sample_ms),
                euler.0,
                euler.1,
                euler.2,
                shaped.0,
                shaped.1,
                compressed.0,
                compressed.1,
                self.vel.0,
                self.vel.1,
                out.0,
                out.1
            );
        }

        vec![MouseCmd::MoveTo(out.0, out.1)]
    }
}

/// Original per-frame shaping: dead zone, then a *linear base* plus a
/// velocity-proportional acceleration term. Its constants are intentionally
/// unchanged because they already had a proven usable feel on the target phone.
fn shape(d: f64, sensitivity: f64) -> f64 {
    let m = d.abs();
    if m < config::REL_DEADZONE {
        return 0.0;
    }
    let e = m - config::REL_DEADZONE;
    d.signum() * sensitivity * e * (1.0 + config::REL_ACCEL * e)
}

/// Radially compress large per-frame motion without changing its direction.
/// Near zero asinh(x) ~= x, so precision movement keeps the original response.
/// Unlike tanh, asinh remains unbounded: increasingly fast flicks still produce
/// increasingly fast cursor travel, but sensor spikes grow only logarithmically.
fn soft_compress_vector(input: (f64, f64), scale: f64) -> (f64, f64) {
    let magnitude = input.0.hypot(input.1);
    if magnitude <= f64::EPSILON {
        return input;
    }
    let compressed_magnitude = scale * (magnitude / scale).asinh();
    let ratio = compressed_magnitude / magnitude;
    (input.0 * ratio, input.1 * ratio)
}

fn wrapped_delta(current: f64, previous: f64) -> f64 {
    let mut delta = current - previous;
    while delta > 180.0 {
        delta -= 360.0;
    }
    while delta < -180.0 {
        delta += 360.0;
    }
    delta
}

fn low_pass_alpha(cutoff_hz: f64, seconds: f64) -> f64 {
    let tau = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz.max(1e-6));
    1.0 / (1.0 + tau / seconds.max(1e-6))
}

/// Pick a 1€ minimum cutoff whose response at 60 Hz equals the old EMA slider.
fn smoothing_to_cutoff(smoothing: f64) -> f64 {
    let response = smoothing.clamp(0.001, 0.999);
    let seconds = 1.0 / config::FILTER_REFERENCE_HZ;
    response / (2.0 * std::f64::consts::PI * seconds * (1.0 - response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_shaping_curve_is_preserved() {
        assert_eq!(shape(0.05, 35.0), 0.0);
        let e = 1.0 - config::REL_DEADZONE;
        assert_eq!(shape(1.0, 35.0), 35.0 * e * (1.0 + config::REL_ACCEL * e));
    }

    #[test]
    fn soft_compression_preserves_direction_and_reduces_large_motion() {
        let input = (60.0, 80.0);
        let output = soft_compress_vector(input, 30.0);
        assert!(output.0.hypot(output.1) < input.0.hypot(input.1));
        assert!(output.0.hypot(output.1) > 30.0);
        assert!((output.0 / output.1 - input.0 / input.1).abs() < 1e-12);
    }

    #[test]
    fn soft_compression_is_nearly_linear_for_precision_motion() {
        let input = (1.0, -2.0);
        let output = soft_compress_vector(input, 30.0);
        assert!((output.0 - input.0).abs() < 0.002);
        assert!((output.1 - input.1).abs() < 0.003);
    }

    #[test]
    fn soft_compression_keeps_fast_flicks_distinguishable() {
        let medium = soft_compress_vector((100.0, 0.0), 30.0).0;
        let fast = soft_compress_vector((500.0, 0.0), 30.0).0;
        assert!(medium > 50.0, "{medium}");
        assert!(fast > 100.0, "{fast}");
        assert!(fast > medium, "{fast} <= {medium}");
    }

    #[test]
    fn one_euro_matches_old_ema_at_reference_speed_then_reacts_faster() {
        let dt = Duration::from_secs_f64(1.0 / config::FILTER_REFERENCE_HZ);
        let cutoff = smoothing_to_cutoff(0.35);
        let mut slow = OneEuroAxis::default();
        let first = slow.filter_delta(10.0, dt, cutoff);
        assert!((first - 3.5).abs() < 1e-6, "{first}");

        let mut fast = OneEuroAxis::default();
        let _ = fast.filter_delta(0.0, dt, cutoff);
        let accelerated = fast.filter_delta(10.0, dt, cutoff);
        assert!(accelerated > first, "{accelerated} <= {first}");
    }

    #[test]
    fn angular_delta_handles_wraparound() {
        assert_eq!(wrapped_delta(1.0, 359.0), 2.0);
        assert_eq!(wrapped_delta(359.0, 1.0), -2.0);
    }
}

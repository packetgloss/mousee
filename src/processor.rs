//! Orientation -> cursor mapping. Holds per-connection state and implements both
//! absolute (calibrated) and relative (air-mouse) modes (SPEC §5, §6).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config;
use crate::monitors::Layout;
use crate::mouse::MouseCmd;
use crate::protocol::{Btn, ClientMsg, Corner, Mode};

/// Computed calibration bounds derived from the 4 corners (SPEC §5.1).
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_beta: f64,   // top
    max_beta: f64,   // bottom
    alpha_left: f64, // unwrapped continuous axis
    alpha_right: f64,
}

pub struct Processor {
    layout: Arc<Layout>,
    debug: bool,

    mode: Mode,
    smoothing: f64,
    anti_jitter: bool,

    // calibration
    calib: HashMap<Corner, (f64, f64)>, // corner -> (beta, alpha)
    bounds: Option<Bounds>,

    // shared smoothed cursor position (both modes write here)
    pos: (f64, f64),
    has_pos: bool,

    // relative mode state
    prev_alpha: Option<f64>,
    prev_beta: Option<f64>,
    vel: (f64, f64),

    // throttled logging
    last_log: HashMap<&'static str, Instant>,
}

impl Processor {
    pub fn new(layout: Arc<Layout>, debug: bool) -> Self {
        Self {
            layout,
            debug,
            mode: Mode::Relative, // air-mouse is the recommended default (SPEC §4.2)
            smoothing: config::DEFAULT_SMOOTHING,
            anti_jitter: false,
            calib: HashMap::new(),
            bounds: None,
            pos: (0.0, 0.0),
            has_pos: false,
            vel: (0.0, 0.0),
            prev_alpha: None,
            prev_beta: None,
            last_log: HashMap::new(),
        }
    }

    /// Reset the per-frame deltas/position so a reconnect or mode switch does
    /// not produce a huge first-frame jump (SPEC §5.2, §10.5).
    fn reset_tracking(&mut self) {
        self.prev_alpha = None;
        self.prev_beta = None;
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
                self.smoothing = value.clamp(0.01, 1.0);
                vec![]
            }
            ClientMsg::AntiJitter { on } => {
                self.anti_jitter = on;
                tracing::info!("anti-jitter -> {on}");
                vec![]
            }
            ClientMsg::Calib { point, beta, alpha } => {
                self.calib.insert(point, (beta, alpha));
                tracing::info!("calib {point:?}: beta={beta:.1} alpha={alpha:.1}");
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
            ClientMsg::Move { beta, alpha, accel, .. } => self.on_move(beta, alpha, accel),
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

    fn on_move(&mut self, beta: f64, alpha: f64, accel: Option<[f64; 3]>) -> Vec<MouseCmd> {
        match self.mode {
            Mode::Absolute => self.absolute(beta, alpha, accel),
            Mode::Relative => self.relative(beta, alpha),
        }
    }

    // --- Absolute (calibrated) mode (SPEC §5.1 + §6) -----------------------
    fn absolute(&mut self, beta: f64, alpha: f64, accel: Option<[f64; 3]>) -> Vec<MouseCmd> {
        let Some(b) = self.bounds else {
            return vec![]; // not calibrated yet
        };

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
        let target_x_f = self.layout.origin_x as f64 + frac_x * self.layout.width as f64;
        let target_x = target_x_f.round() as i32;

        // Vertical is stretched over the REAL monitor under the pointer (§6).
        let mon = *self.layout.monitor_for_x(target_x);
        let cx = target_x.clamp(mon.x, mon.x + mon.w - 1);
        let cy_f = mon.y as f64 + frac_y * mon.h as f64;
        let cy = (cy_f.round() as i32).clamp(mon.y, mon.y + mon.h - 1);
        let target = (cx as f64, cy as f64);

        // EMA smoothing, with dynamic anti-jitter reducing the factor (§5.1).
        let mut sf = self.smoothing;
        if self.anti_jitter {
            if let Some([ax, ay, az]) = accel {
                let mag = (ax * ax + ay * ay + az * az).sqrt();
                let jitter = (mag - 9.8).abs();
                if jitter > config::ANTIJITTER_THRESHOLD {
                    sf = config::ANTIJITTER_SMOOTHING;
                }
            }
        }

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
    fn relative(&mut self, beta: f64, alpha: f64) -> Vec<MouseCmd> {
        // Need two samples to take a delta; first frame only primes the state.
        let (Some(pa), Some(pb)) = (self.prev_alpha, self.prev_beta) else {
            self.prev_alpha = Some(alpha);
            self.prev_beta = Some(beta);
            return vec![];
        };

        // alpha wraps at 0/360: bring delta into [-180, 180].
        let mut da = alpha - pa;
        while da > 180.0 {
            da -= 360.0;
        }
        while da < -180.0 {
            da += 360.0;
        }
        let db = beta - pb;
        self.prev_alpha = Some(alpha);
        self.prev_beta = Some(beta);

        let dx = config::REL_SIGN_X * shape(da);
        let dy = config::REL_SIGN_Y * shape(db);

        // Low-pass the velocity for smoothness.
        let s = config::REL_VEL_SMOOTH;
        self.vel.0 = self.vel.0 * (1.0 - s) + dx * s;
        self.vel.1 = self.vel.1 * (1.0 - s) + dy * s;

        let (ox, oy, w, h) = (
            self.layout.origin_x,
            self.layout.origin_y,
            self.layout.width,
            self.layout.height,
        );
        if !self.has_pos {
            // Start from the center of the virtual desktop.
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
            tracing::debug!("rel: da={da:.2} db={db:.2} v=({:.1},{:.1}) pos=({},{})", self.vel.0, self.vel.1, out.0, out.1);
        }

        vec![MouseCmd::MoveTo(out.0, out.1)]
    }
}

/// Shaping function for a per-frame delta: dead zone + power-curve acceleration.
fn shape(d: f64) -> f64 {
    let m = d.abs();
    if m < config::REL_DEADZONE {
        return 0.0;
    }
    d.signum() * (m - config::REL_DEADZONE).powf(config::REL_EXP) * config::REL_SENSITIVITY
}

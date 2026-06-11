//! Multi-monitor geometry & the virtual desktop bounding box (SPEC §6).

use crate::config;

#[derive(Debug, Clone, Copy)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Monitor {
    #[inline]
    fn contains_x(&self, x: i32) -> bool {
        x >= self.x && x < self.x + self.w
    }
    #[inline]
    fn center_x(&self) -> i32 {
        self.x + self.w / 2
    }
}

/// The combined virtual desktop: bounding box over all monitors plus the
/// individual monitor rectangles (needed for the per-monitor vertical mapping).
#[derive(Debug, Clone)]
pub struct Layout {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: i32,
    pub height: i32,
    pub monitors: Vec<Monitor>,
}

impl Layout {
    /// Read the real layout from the OS, falling back to a single 1920x1080
    /// screen if enumeration fails (soft degradation, SPEC §2.5).
    pub fn detect() -> Self {
        match display_info::DisplayInfo::all() {
            Ok(list) if !list.is_empty() => {
                let monitors: Vec<Monitor> = list
                    .iter()
                    .map(|d| Monitor {
                        x: d.x,
                        y: d.y,
                        w: d.width as i32,
                        h: d.height as i32,
                    })
                    .collect();
                Self::from_monitors(monitors)
            }
            other => {
                if let Err(e) = other {
                    tracing::warn!("monitor enumeration failed ({e}); using fallback 1920x1080");
                } else {
                    tracing::warn!("no monitors reported; using fallback 1920x1080");
                }
                Self::from_monitors(vec![Monitor {
                    x: 0,
                    y: 0,
                    w: config::FALLBACK_WIDTH,
                    h: config::FALLBACK_HEIGHT,
                }])
            }
        }
    }

    fn from_monitors(monitors: Vec<Monitor>) -> Self {
        let origin_x = monitors.iter().map(|m| m.x).min().unwrap_or(0);
        let origin_y = monitors.iter().map(|m| m.y).min().unwrap_or(0);
        let max_x = monitors.iter().map(|m| m.x + m.w).max().unwrap_or(config::FALLBACK_WIDTH);
        let max_y = monitors.iter().map(|m| m.y + m.h).max().unwrap_or(config::FALLBACK_HEIGHT);
        Self {
            origin_x,
            origin_y,
            width: (max_x - origin_x).max(1),
            height: (max_y - origin_y).max(1),
            monitors,
        }
    }

    /// Pick the monitor whose X-range contains `x`; otherwise the one whose
    /// center is nearest (SPEC §6 step 2).
    pub fn monitor_for_x(&self, x: i32) -> &Monitor {
        if let Some(m) = self.monitors.iter().find(|m| m.contains_x(x)) {
            return m;
        }
        self.monitors
            .iter()
            .min_by_key(|m| (m.center_x() - x).abs())
            .expect("layout always has >= 1 monitor")
    }

    pub fn log_summary(&self) {
        tracing::info!(
            "virtual desktop: origin=({}, {}) size={}x{} over {} monitor(s)",
            self.origin_x,
            self.origin_y,
            self.width,
            self.height,
            self.monitors.len()
        );
        for (i, m) in self.monitors.iter().enumerate() {
            tracing::info!("  monitor #{i}: pos=({}, {}) size={}x{}", m.x, m.y, m.w, m.h);
        }
    }
}

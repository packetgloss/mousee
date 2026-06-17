//! Multi-monitor geometry & the virtual desktop bounding box (SPEC §6).

use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// Cheap structural equality used by the hotplug watcher to decide whether
    /// anything actually changed (monitor added/removed/moved/resized).
    fn same_geometry(&self, other: &Layout) -> bool {
        self.origin_x == other.origin_x
            && self.origin_y == other.origin_y
            && self.width == other.width
            && self.height == other.height
            && self.monitors == other.monitors
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

/// How often the background thread re-polls the OS for monitor changes.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// A hot-swappable handle to the current [`Layout`]. The OS layout can change at
/// runtime (a monitor is plugged in, unplugged, or rearranged); a background
/// thread re-detects it and atomically swaps in the new geometry, so the cursor
/// mapping always reflects the live virtual desktop (SPEC §6).
#[derive(Clone)]
pub struct LayoutHandle {
    inner: Arc<RwLock<Arc<Layout>>>,
}

impl LayoutHandle {
    /// Detect the layout once and wrap it. Call [`spawn_watcher`] to keep it live.
    pub fn detect() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(Layout::detect()))),
        }
    }

    /// Snapshot the current layout (cheap `Arc` clone). Callers hold this for the
    /// duration of a single frame so a mid-frame swap can't tear the mapping.
    pub fn current(&self) -> Arc<Layout> {
        self.inner.read().expect("layout lock poisoned").clone()
    }

    fn store(&self, layout: Arc<Layout>) {
        *self.inner.write().expect("layout lock poisoned") = layout;
    }

    /// Spawn the hotplug watcher thread. Polls the OS every [`WATCH_INTERVAL`]
    /// and swaps the layout in (logging the new geometry) whenever it changes.
    pub fn spawn_watcher(&self) {
        let handle = self.clone();
        std::thread::Builder::new()
            .name("mousee-monitors".into())
            .spawn(move || loop {
                std::thread::sleep(WATCH_INTERVAL);
                let fresh = Layout::detect();
                if !fresh.same_geometry(&handle.current()) {
                    tracing::info!("monitor layout changed — updating virtual desktop");
                    fresh.log_summary();
                    handle.store(Arc::new(fresh));
                }
            })
            .expect("spawning monitor watcher thread");
    }
}

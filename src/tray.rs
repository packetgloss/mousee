//! System-tray UX (SPEC §12.3). Optional, behind the `tray` feature.
//!
//! This runs in the *background worker* process (spawned detached, with no
//! console of its own), so closing the launcher console does not affect it.
//! tray-icon needs a native event loop on the main thread, so this takes over
//! the main thread; the tokio server runs on its own runtime thread.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// Show the connection QR. The worker has no console of its own, so spawn a
/// fresh console window (a hidden `--show-qr` mode of this same binary) that
/// prints the QR + URL and waits. Closing that window does not affect the daemon.
#[cfg(windows)]
fn show_qr(url: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .arg("--show-qr")
            .arg("--url")
            .arg(url)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn();
    }
}

#[cfg(not(windows))]
fn show_qr(url: &str) {
    println!("Address: {url}");
}

/// The tray icon: a 32x32 RGBA buffer decoded from `src/icon.png` at build time
/// (see build.rs), embedded directly so the runtime needs no image crate.
fn build_icon() -> Option<Icon> {
    const SIZE: u32 = 32;
    let rgba = include_bytes!(concat!(env!("OUT_DIR"), "/tray.rgba")).to_vec();
    Icon::from_rgba(rgba, SIZE, SIZE).ok()
}

fn tooltip(ip: Ipv4Addr, port: u16, connected: bool) -> String {
    if connected {
        format!("mousee — phone connected ({ip}:{port})")
    } else {
        format!("mousee — waiting for phone ({ip}:{port})")
    }
}

/// Run the tray event loop. Blocks the calling (main) thread until "Quit".
pub fn run(url: String, ip: Ipv4Addr, port: u16, connected: Arc<AtomicBool>) -> ! {
    let event_loop: EventLoop<()> = EventLoop::new();

    let menu = Menu::new();
    let header = MenuItem::new(format!("mousee  {ip}:{port}"), false, None);
    let qr = MenuItem::new("Show QR", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let _ = menu.append(&header);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&qr);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let qr_id = qr.id().clone();
    let quit_id = quit.id().clone();

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(tooltip(ip, port, false));
    if let Some(icon) = build_icon() {
        builder = builder.with_icon(icon);
    }

    let tray = match builder.build() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("could not create tray icon ({e}); running headless");
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
    };

    let menu_rx = MenuEvent::receiver();
    let mut last_connected = false;

    event_loop.run(move |_event, _target, control_flow| {
        // Wake periodically to refresh the status tooltip.
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(750));

        let now = connected.load(Ordering::Relaxed);
        if now != last_connected {
            last_connected = now;
            let _ = tray.set_tooltip(Some(tooltip(ip, port, now)));
        }

        while let Ok(ev) = menu_rx.try_recv() {
            if ev.id == qr_id {
                show_qr(&url);
            } else if ev.id == quit_id {
                std::process::exit(0);
            }
        }
    })
}

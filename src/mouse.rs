//! Dedicated mouse-control thread. `enigo` is not `Send`/async-friendly, so we
//! keep one OS thread that owns the `Enigo` handle and consumes commands from a
//! channel. Soft degradation: if the device can't be captured we drop commands
//! instead of crashing (SPEC §2.5).

use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Mouse, Settings};

use crate::protocol::Btn;

#[derive(Debug, Clone, Copy)]
pub enum MouseCmd {
    MoveTo(i32, i32),
    Press(Btn),
    Release(Btn),
    /// Vertical wheel, in (already signed) ticks.
    Scroll(i32),
}

/// Spawn the mouse thread and return a sender for commands.
pub fn spawn() -> Sender<MouseCmd> {
    let (tx, rx) = std::sync::mpsc::channel::<MouseCmd>();
    thread::Builder::new()
        .name("mouse".into())
        .spawn(move || run(rx))
        .expect("failed to spawn mouse thread");
    tx
}

fn run(rx: Receiver<MouseCmd>) {
    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(e) => Some(e),
        Err(e) => {
            tracing::error!("could not initialise input device ({e}); cursor control disabled");
            None
        }
    };

    while let Ok(cmd) = rx.recv() {
        let Some(enigo) = enigo.as_mut() else {
            continue; // degraded mode: silently drop
        };
        if let Err(e) = apply(enigo, cmd) {
            tracing::debug!("input command failed: {e}");
        }
    }
}

fn button(b: Btn) -> Button {
    match b {
        Btn::Left => Button::Left,
        Btn::Right => Button::Right,
    }
}

fn apply(enigo: &mut Enigo, cmd: MouseCmd) -> Result<(), enigo::InputError> {
    match cmd {
        MouseCmd::MoveTo(x, y) => enigo.move_mouse(x, y, Coordinate::Abs),
        MouseCmd::Press(b) => enigo.button(button(b), Direction::Press),
        MouseCmd::Release(b) => enigo.button(button(b), Direction::Release),
        MouseCmd::Scroll(ticks) => enigo.scroll(ticks, Axis::Vertical),
    }
}

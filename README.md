# mousee

Turn your phone into an air-mouse / laser pointer for your PC. The phone's
browser streams device-orientation data over a WebSocket to a small Rust daemon
on the PC, which moves the system cursor.

Implements [`SPEC.md`](./SPEC.md). Two artifacts only:

- **Client** — a single self-contained HTML file (`client/index.html`), embedded
  into the binary at build time via `include_str!`.
- **Server** — one Rust binary (`mousee`). No external runtime, no system OpenSSL.

## Build

Requires a Rust toolchain (stable). On Windows install via
[rustup](https://rustup.rs).

```powershell
cargo build --release
```

The release binary is `target/release/mousee(.exe)`. It is fully self-contained —
the HTML client is baked in.

## Run

```powershell
cargo run --release          # interactive: pick interface, prints a QR code
.\target\release\mousee.exe  # same, from the built binary
```

On start the server:

1. reads the monitor layout and computes the virtual desktop (logged);
2. lets you pick the network interface (arrow keys, recommended one preselected);
3. generates/loads a self-signed TLS certificate for the chosen IP (cached in
   `./mousee-cert/`, reissued when the IP changes);
4. binds `0.0.0.0:<port>` for **both** the page and the WebSocket;
5. prints a QR code with `https://<ip>:<port>`.

Scan the QR with your phone, **accept the self-signed certificate once**, tap
**Connect** (this grants motion sensors on iOS), then pick a mode.

### Flags

| Flag | Effect |
|---|---|
| `--ip <IPV4>` | Force the advertised LAN-IP, skip the picker. |
| `--port <N>` | Port for page + WebSocket (default `8081`). |
| `--yes` / `--no-tui` | Headless: use the recommended/forced IP, no picker, no tray. |
| `--no-tls` | Serve plain HTTP (⚠ iOS will **not** grant sensors). |
| `--no-tray` | Don't show the tray icon; run in the foreground until Ctrl-C. |
| `--debug` | Verbose, throttled per-frame mapping logs. |

## System tray & background (Windows)

Closing a console window kills its process — Windows offers no way around that.
So mousee uses two processes:

1. The **launcher** (this console) picks the interface, prints the QR, then
   spawns…
2. a **detached, console-less background worker** that runs the server and the
   tray icon.

That means you can **close the launcher console with the X** and the app keeps
running in the tray — exactly like a normal background app. Quit from the tray
icon → **Quit**.

Tray menu:

- a header showing the current `IP:port`,
- **Show QR** — opens a fresh console window with the QR + URL (closing it does
  not affect the running daemon),
- **Quit**.

The tooltip switches between "waiting for phone" and "phone connected".

Use `--no-tray` to instead stay in the foreground (server + logs in this console,
Ctrl-C to quit) — handy for debugging or running under a service manager.
Build without it via `cargo build --release --no-default-features` (or run with
`--no-tray`).

## Modes

- **Air Mouse (relative)** — recommended, no calibration. Aim to move; movement is
  by orientation *delta* per frame with a dead zone + acceleration curve.
- **Absolute** — aim maps directly to the screen after a 4-corner calibration.
  Horizontal spans the whole virtual desktop; vertical is stretched over the real
  monitor under the pointer (no dead zones on mixed-height multi-monitor setups).

## Gestures (control screen)

- **Tap** — click (left half = LMB, right half = RMB).
- **Hold still, then aim** — presses & holds the button for drag/select.
- **Swipe** — scroll wheel (TikTok-style).

## Tuning

All knobs are explicit constants at the top of the files:

- Server: [`src/config.rs`](./src/config.rs).
- Client: the `CFG` object and `:root` CSS vars in
  [`client/index.html`](./client/index.html).

## Notes / known limitations

- **Linux/Wayland:** input injection (`enigo`) is restricted under Wayland; X11
  works. Primary target is Windows. The tray (`tray-icon` + `tao`) is also best
  tested on Windows; disable with `--no-default-features` if it fails to build.
- **Autostart at login** (SPEC §12.3) is not wired up; the tray itself is.
- `alpha` is a compass/magnetometer reading: absolute horizontal is inherently
  less stable than relative and needs calibration. This is hardware, not a bug.

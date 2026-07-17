//! Build script: derive every icon target from the single source `src/icon.png`.
//!
//! * `tray.rgba` — 32x32 raw RGBA, decoded at compile time so the runtime needs
//!   no image crate (used by the system-tray icon).
//! * `favicon.png` — 48x48 PNG served by the web client.
//! * `icon.ico` — multi-size Windows icon, embedded into the `.exe` so it shows
//!   up in Explorer / the taskbar (Windows only).

use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.png");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out = Path::new(&out_dir);
    let src = image::open("src/icon.png")
        .expect("decoding src/icon.png")
        .to_rgba8();

    // Tray icon: 32x32 raw RGBA buffer.
    let tray = image::imageops::resize(&src, 32, 32, image::imageops::FilterType::Lanczos3);
    std::fs::write(out.join("tray.rgba"), tray.as_raw()).expect("writing tray.rgba");

    // Favicon: 48x48 PNG for the web client.
    let favicon = image::imageops::resize(&src, 48, 48, image::imageops::FilterType::Lanczos3);
    favicon
        .save(out.join("favicon.png"))
        .expect("writing favicon.png");

    // Apple touch icon: 180x180 PNG used when iOS "Add to Home Screen" saves the
    // page. iOS ignores the favicon and needs this exact link/size, else it
    // falls back to a letter glyph.
    let apple = image::imageops::resize(&src, 180, 180, image::imageops::FilterType::Lanczos3);
    apple
        .save(out.join("apple-touch-icon.png"))
        .expect("writing apple-touch-icon.png");

    // Windows .exe icon: a multi-size .ico, embedded as a resource.
    #[cfg(windows)]
    {
        let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
        for size in [16u32, 32, 48, 64, 128, 256] {
            let resized =
                image::imageops::resize(&src, size, size, image::imageops::FilterType::Lanczos3);
            let img = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
            icon_dir.add_entry(ico::IconDirEntry::encode(&img).expect("encoding ico entry"));
        }
        let ico_path = out.join("icon.ico");
        let file = std::fs::File::create(&ico_path).expect("creating icon.ico");
        icon_dir.write(file).expect("writing icon.ico");

        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().expect("icon.ico path is not UTF-8"));
        res.compile().expect("embedding Windows icon resource");
    }
}

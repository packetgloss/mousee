//! Per-user Windows autostart choice for release/tray launches.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Select};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "mousee";
const DISMISSED_FILE: &str = "autostart-prompt-dismissed";

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { RegCloseKey(self.0) };
        }
    }
}

fn open_run_key(access: u32) -> Result<Key> {
    let path = wide(RUN_KEY);
    let mut key = std::ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32))
            .context("opening the current-user Windows Run key");
    }
    Ok(Key(key))
}

fn is_enabled() -> Result<bool> {
    let key = open_run_key(KEY_QUERY_VALUE)?;
    let name = wide(VALUE_NAME);
    let mut kind = 0;
    let mut bytes = 0;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    Ok(status == ERROR_SUCCESS && kind == REG_SZ && bytes > 2)
}

fn powershell_literal(value: &Path) -> String {
    value.to_string_lossy().replace('\'', "''")
}

fn autostart_command(exe: &Path) -> String {
    // Explorer starts executables from the Run key without creation flags. A
    // console-subsystem binary therefore gets an empty console before main()
    // can hide it, and closing that console terminates the tray worker. Let the
    // built-in Windows launcher create the worker hidden from the outset.
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden \
         -Command \"Start-Process -FilePath '{}' -ArgumentList '--background' \
         -WindowStyle Hidden\"",
        powershell_literal(exe)
    )
}

fn enable() -> Result<()> {
    let exe = std::env::current_exe().context("locating mousee.exe")?;
    // --background auto-detects the current LAN address, so the Run entry
    // remains valid when the network changes.
    let command = autostart_command(&exe);
    let value = wide(command);
    let bytes = unsafe {
        std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * size_of::<u16>())
    };
    let key = open_run_key(KEY_SET_VALUE)?;
    let name = wide(VALUE_NAME);
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32))
            .context("adding mousee to Windows autostart");
    }
    Ok(())
}

fn dismissed_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|p| p.join("mousee").join(DISMISSED_FILE))
}

fn dismiss_forever() -> Result<()> {
    let path = dismissed_path().context("LOCALAPPDATA is unavailable")?;
    let parent = path.parent().context("invalid autostart preference path")?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(path, b"dismissed\n")?;
    Ok(())
}

/// Ask once per launch unless the user permanently dismisses the question or
/// autostart is already configured. The first item is deliberately the default.
pub fn prompt() -> Result<()> {
    if is_enabled()? {
        // Refresh the executable path after the user moves/upgrades the binary.
        enable()?;
        return Ok(());
    }
    if dismissed_path().is_some_and(|p| p.exists()) {
        return Ok(());
    }

    let choices = [
        "No, not now (default)",
        "No, and don't ask again",
        "Yes, start mousee with Windows",
    ];
    let selected = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Start mousee automatically when you sign in to Windows?")
        .items(choices)
        .default(0)
        .interact_opt()?
        .unwrap_or(0);

    match selected {
        1 => {
            dismiss_forever()?;
            println!("Autostart question disabled.\n");
        }
        2 => {
            enable()?;
            println!("mousee was added to Windows autostart.\n");
        }
        _ => println!("Autostart was not changed.\n"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::autostart_command;
    use std::path::Path;

    #[test]
    fn autostart_launches_the_worker_hidden() {
        let command = autostart_command(Path::new(r"C:\Users\O'Brien\mousee.exe"));

        assert!(command.starts_with("powershell.exe "));
        assert!(command.contains("-WindowStyle Hidden"));
        assert!(command.contains("-ArgumentList '--background'"));
        assert!(command.contains(r"-FilePath 'C:\Users\O''Brien\mousee.exe'"));
    }
}

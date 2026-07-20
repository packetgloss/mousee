//! Single-instance guard.
//!
//! The detached background worker owns a named mutex for its whole lifetime;
//! the interactive launcher checks for that mutex and bails out if a worker is
//! already running, so double-clicking the exe again does not spin up a second
//! tray icon / server (which would fail to bind the port anyway).

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    // Per-session name (random suffix avoids clashing with anything else).
    const NAME: &str = "Local\\mousee-singleton-9e1c4f";

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Keeps the singleton mutex alive for the lifetime of the process.
    pub struct Guard(HANDLE);

    // The HANDLE is only ever closed on drop; safe to move across threads.
    unsafe impl Send for Guard {}

    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    /// Try to become the single instance. `Some(guard)` means we own it; hold the
    /// guard for as long as this process should be considered "the instance".
    /// `None` means another instance already owns the mutex.
    pub fn acquire() -> Option<Guard> {
        let name = wide(NAME);
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            // Could not create the object — don't block startup over it.
            return Some(Guard(handle));
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(handle) };
            None
        } else {
            Some(Guard(handle))
        }
    }

    /// Non-owning check: is another instance already running? (Acquires and then
    /// immediately releases our own handle, so it never keeps the mutex alive.)
    #[cfg(feature = "tray")]
    pub fn is_running() -> bool {
        acquire().is_none()
    }

    /// Native "already running" notification: a standard Windows message box.
    /// Hides this launcher's console window first, so the *only* thing the user
    /// sees is the dialog (the console would otherwise flash behind it).
    #[cfg(feature = "tray")]
    pub fn warn_already_running() {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, ShowWindow, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, SW_HIDE,
        };
        unsafe {
            let console = GetConsoleWindow();
            if !console.is_null() {
                ShowWindow(console, SW_HIDE);
            }
        }
        let text = wide(
            "mousee is already running.\n\n\
             Look for the cursor icon in the system tray (bottom-right).",
        );
        let caption = wide("mousee");
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    /// Attach a release launcher to its caller's terminal or create one when
    /// started from Explorer. Background workers never call this function.
    #[cfg(not(debug_assertions))]
    pub fn ensure_console() {
        use windows_sys::Win32::System::Console::{
            AllocConsole, AttachConsole, ATTACH_PARENT_PROCESS,
        };
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                let _ = AllocConsole();
            }
        }
    }

    /// Kept for a worker launched by an older Run entry. New release workers
    /// use the GUI subsystem and arrive without a console.
    pub fn hide_console() {
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe {
            let console = GetConsoleWindow();
            if !console.is_null() {
                ShowWindow(console, SW_HIDE);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub struct Guard;
    pub fn acquire() -> Option<Guard> {
        Some(Guard)
    }
    pub fn hide_console() {}
}

#[cfg(all(windows, not(debug_assertions)))]
pub use imp::ensure_console;
pub use imp::{acquire, hide_console};
#[cfg(all(windows, feature = "tray"))]
pub use imp::{is_running, warn_already_running};

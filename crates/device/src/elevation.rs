//! Administrator rights (UAC).

use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr;

use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use super::ioctl::wide;

/// Returns whether the process runs with administrator rights.
#[must_use]
pub fn is_elevated() -> bool {
    // SAFETY: `IsUserAnAdmin` takes no arguments.
    unsafe { IsUserAnAdmin() != 0 }
}

/// Starts this executable again with administrator rights (UAC prompt).
///
/// The caller should exit when this succeeds.
///
/// # Errors
///
/// Returns an error when the path of the executable cannot be determined or
/// the user declined the UAC prompt.
pub fn relaunch_elevated() -> io::Result<()> {
    let exe: Vec<u16> = std::env::current_exe()?
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let verb = wide("runas");
    // SAFETY: all strings are NUL terminated UTF-16; null pointers are
    // allowed for the window, parameters and directory.
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            exe.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values greater than 32 indicate success.
    if result as usize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(
            "the program was not started with administrator rights",
        ))
    }
}

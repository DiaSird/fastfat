//! Thin wrappers around `DeviceIoControl` and UTF-16 strings.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::ptr;

use windows_sys::Win32::System::IO::DeviceIoControl;

/// Issues `DeviceIoControl` with raw buffers.
///
/// # Safety
///
/// `input` must be valid for reads of `input_len` bytes and `output` valid for
/// writes of `output_len` bytes (both may be null with a length of 0).
pub(super) unsafe fn device_io_control(
    file: &File,
    code: u32,
    input: *const c_void,
    input_len: usize,
    output: *mut c_void,
    output_len: usize,
) -> io::Result<()> {
    let mut returned = 0_u32;
    // SAFETY: the caller guarantees the buffers; the handle is owned by `file`
    // and the call is synchronous (no OVERLAPPED).
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            input,
            u32::try_from(input_len).map_err(io::Error::other)?,
            output,
            u32::try_from(output_len).map_err(io::Error::other)?,
            &raw mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Issues a control code without input or output buffers.
pub(super) fn ioctl_none(file: &File, code: u32) -> io::Result<()> {
    // SAFETY: no buffers are passed.
    unsafe { device_io_control(file, code, ptr::null(), 0, ptr::null_mut(), 0) }
}

/// Issues a control code that fills a plain-data output structure.
pub(super) fn ioctl_query<T: Default>(file: &File, code: u32) -> io::Result<T> {
    let mut out = T::default();
    // SAFETY: `out` is a valid, writable `T`; the Win32 structures used here are
    // plain data for which every bit pattern written by the driver is valid.
    unsafe {
        device_io_control(
            file,
            code,
            ptr::null(),
            0,
            (&raw mut out).cast(),
            size_of::<T>(),
        )?;
    };
    Ok(out)
}

/// Issues a control code that takes a plain-data input structure.
pub(super) fn ioctl_set<T>(file: &File, code: u32, input: &T) -> io::Result<()> {
    // SAFETY: `input` is a valid `T` for the duration of the call.
    unsafe {
        device_io_control(
            file,
            code,
            ptr::from_ref(input).cast(),
            size_of::<T>(),
            ptr::null_mut(),
            0,
        )
    }
}

/// Adds a human readable message in front of an I/O error.
pub(super) fn context(err: io::Error, message: &str) -> io::Error {
    io::Error::new(err.kind(), format!("{message}: {err}"))
}

/// Converts a string to a NUL terminated UTF-16 buffer.
pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Converts a NUL terminated UTF-16 buffer to a string.
pub(super) fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

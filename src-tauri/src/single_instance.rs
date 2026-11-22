//! One GUI instance per signed-in user. A second launch must not run startup session cleanup
//! against the first instance's live TUN session.

#![allow(unsafe_code, reason = "Win32 named mutex and message box")]

use std::sync::OnceLock;
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
    System::Threading::CreateMutexW,
    UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW},
};

static INSTANCE_MUTEX: OnceLock<usize> = OnceLock::new();

pub fn acquire() -> bool {
    let name: Vec<u16> = "Local\\Clashfork.SingleInstance.v1"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return true;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let message: Vec<u16> =
            "Clashfork is already running. Use the existing window or its tray icon."
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
        let title: Vec<u16> = "Clashfork"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
        return false;
    }
    let _ = INSTANCE_MUTEX.set(handle as usize);
    true
}

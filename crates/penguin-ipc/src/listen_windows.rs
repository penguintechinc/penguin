//! Windows named-pipe listener for the daemon's control socket.
//!
//! Unlike the Unix listener, there is no per-RPC peer check here: the named
//! pipe's DACL — set once, at creation time, from [`PIPE_SDDL`] — is the
//! entire authorization boundary. This asymmetry with the Unix side is
//! intentional, matching the frozen Go reference
//! (`go-client/internal/ipc/listen_windows.go`): Windows named pipes do not
//! expose a per-connection peer identity the way `SO_PEERCRED` does without
//! additional impersonation-token plumbing, and the DACL already restricts
//! pipe access to Builtin Administrators and SYSTEM at the OS level. Do not
//! add a peer check here to "match" the Unix side — there is nothing for it
//! to check.
//!
//! This file is not compiled or verified on Linux CI — `#[cfg(windows)]` on
//! its `pub mod` declaration in `lib.rs` excludes it entirely from a Linux
//! build. It is verified by the Windows job introduced in M7.

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;

/// The daemon's well-known named pipe path.
pub const PIPE_PATH: &str = r"\\.\pipe\penguind";

/// SDDL for the pipe's DACL: protected (`P`, no inheritance), granting
/// `GENERIC_ALL` only to Builtin Administrators (`BA`) and SYSTEM (`SY`).
/// This is the sole authorization boundary on Windows — see the module doc.
const PIPE_SDDL: &str = "D:P(A;;GA;;;BA)(A;;GA;;;SY)";

/// Binds the daemon's named pipe at [`PIPE_PATH`] with [`PIPE_SDDL`] as its
/// DACL.
pub fn listen() -> io::Result<NamedPipeServer> {
    let security_descriptor = parse_sddl(PIPE_SDDL)?;

    let mut security_attributes = SECURITY_ATTRIBUTES {
        nLength: 0,
        lpSecurityDescriptor: security_descriptor,
        bInheritHandle: 0,
    };
    // `nLength` must be the struct's own size; filled in after construction
    // so the type is inferred from `security_attributes` itself rather than
    // spelled out a second time via turbofish.
    security_attributes.nLength = std::mem::size_of_val(&security_attributes) as u32;

    // SAFETY: `security_attributes` is a valid, live `SECURITY_ATTRIBUTES`
    // value for the duration of this call, and its `lpSecurityDescriptor`
    // points at the descriptor `parse_sddl` just allocated — this satisfies
    // `create_with_security_attributes_raw`'s safety contract (a null or
    // valid `SECURITY_ATTRIBUTES` pointer).
    #[allow(unsafe_code)]
    let result = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .create_with_security_attributes_raw(PIPE_PATH, (&raw mut security_attributes).cast())
    };

    // SAFETY: `security_descriptor` was allocated by
    // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which documents
    // `LocalFree` as the required release call for its result.
    #[allow(unsafe_code)]
    unsafe {
        LocalFree(security_descriptor);
    }

    result
}

/// Parses `sddl` into a heap-allocated `SECURITY_DESCRIPTOR`, returning the
/// raw pointer `SECURITY_ATTRIBUTES::lpSecurityDescriptor` expects. The
/// caller owns the returned pointer and must release it with `LocalFree`.
fn parse_sddl(sddl: &str) -> io::Result<*mut c_void> {
    let mut sddl_wide: Vec<u16> = OsStr::new(sddl).encode_wide().collect();
    sddl_wide.push(0);

    let mut security_descriptor: *mut c_void = ptr::null_mut();
    // SAFETY: narrowly scoped to the single FFI call needed to parse the
    // fixed SDDL string above. `sddl_wide` is a valid null-terminated UTF-16
    // buffer for the duration of the call, and `security_descriptor` is
    // freed by the caller via `LocalFree` on success.
    #[allow(unsafe_code)]
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_ptr(),
            SDDL_REVISION_1,
            &mut security_descriptor,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(security_descriptor)
}

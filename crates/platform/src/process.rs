//! Process helpers: spawn a detached helper, wait for a process to exit.

use crate::bindings::*;
use crate::wide::to_wide;
use windows_core::{Error, PCWSTR, PWSTR, Result};

const SYNCHRONIZE: u32 = 0x0010_0000;

/// `CreateProcessW` flag: the new process is not put into the job object of its creator. Not in
/// the generated bindings; the value is from `winbase.h`.
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Blocks until the process with `pid` exits. Returns immediately if it cannot be opened.
pub fn wait_for_process_exit(pid: u32) {
    // SAFETY: plain FFI calls; the handle is closed afterwards.
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, false, pid);
        if h.0.is_null() {
            return;
        }
        let _ = WaitForSingleObject(h, INFINITE);
        let _ = CloseHandle(h);
    }
}

pub fn current_pid() -> u32 {
    // SAFETY: plain FFI call.
    unsafe { GetCurrentProcessId() }
}

/// Lets any process take the foreground window from this one, e.g. the running instance that
/// a second launch asks to show its settings. Only works while this process may set it.
pub fn allow_foreground_handoff() {
    // SAFETY: plain FFI call; fails harmlessly without the foreground right.
    let _ =
        unsafe { crate::store_bindings::AllowSetForegroundWindow(crate::store_bindings::ASFW_ANY) };
}

/// How long ago the process at the server end of `pipe` (a connected client handle) started.
pub fn pipe_server_age(pipe: &std::fs::File) -> Option<std::time::Duration> {
    use std::os::windows::io::AsRawHandle;
    windows_core::link!("kernel32.dll" "system" fn GetNamedPipeServerProcessId(pipe: HANDLE, pid: *mut u32) -> windows_core::BOOL);
    windows_core::link!("kernel32.dll" "system" fn GetProcessTimes(process: HANDLE, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> windows_core::BOOL);
    windows_core::link!("kernel32.dll" "system" fn GetSystemTimeAsFileTime(now: *mut u64));
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    let (mut pid, mut created, mut now, mut unused) = (0u32, 0u64, 0u64, 0u64);
    // SAFETY: plain FFI calls on a live pipe handle; FILETIMEs are written as u64 (same layout);
    // the process handle is closed before returning.
    unsafe {
        GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut pid)
            .ok()
            .ok()?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if process.0.is_null() {
            return None;
        }
        let ok = GetProcessTimes(process, &mut created, &mut unused, &mut unused, &mut unused);
        let _ = CloseHandle(process);
        ok.ok().ok()?;
        GetSystemTimeAsFileTime(&mut now);
    }
    // FILETIME counts 100 ns intervals.
    Some(std::time::Duration::from_nanos(
        now.saturating_sub(created) * 100,
    ))
}

/// Whether process `pid` runs elevated (as administrator). `None` when its token cannot be
/// read, which for another user's or an elevated process is the usual answer.
pub fn is_elevated(pid: u32) -> Option<bool> {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    // TOKEN_INFORMATION_CLASS::TokenElevation; the answer is a TOKEN_ELEVATION (one u32).
    const TOKEN_ELEVATION_CLASS: TOKEN_INFORMATION_CLASS = 20;
    // SAFETY: plain FFI calls; the process (unless it is the pseudo handle) and token handles
    // are closed before returning.
    unsafe {
        let own = pid == GetCurrentProcessId();
        let process = if own {
            GetCurrentProcess()
        } else {
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
        };
        if process.0.is_null() {
            return None;
        }
        let mut token = HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY as u32, &mut token).as_bool();
        if !own {
            let _ = CloseHandle(process);
        }
        if !opened {
            return None;
        }
        let (mut elevated, mut len) = (0u32, 0u32);
        let ok = GetTokenInformation(
            token,
            TOKEN_ELEVATION_CLASS,
            Some((&mut elevated as *mut u32).cast()),
            4,
            &mut len,
        )
        .as_bool();
        let _ = CloseHandle(token);
        ok.then_some(elevated != 0)
    }
}

/// Seconds from the start of process `pid` to the start of this one (negative when `pid`
/// started later), e.g. how long after Explorer a launch at sign-in came.
pub fn seconds_after_start_of(pid: u32) -> Option<f64> {
    windows_core::link!("kernel32.dll" "system" fn GetProcessTimes(process: HANDLE, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> windows_core::BOOL);
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    // SAFETY: FILETIMEs are written as u64 (same layout) into locals.
    let created = |process: HANDLE| unsafe {
        let (mut created, mut unused) = (0u64, 0u64);
        GetProcessTimes(process, &mut created, &mut unused, &mut unused, &mut unused)
            .as_bool()
            .then_some(created)
    };
    // SAFETY: plain FFI calls; the opened process handle is closed before returning.
    let theirs = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if process.0.is_null() {
            return None;
        }
        let at = created(process);
        let _ = CloseHandle(process);
        at?
    };
    // SAFETY: GetCurrentProcess is a pseudo handle that needs no closing.
    let ours = created(unsafe { GetCurrentProcess() })?;
    // FILETIME counts 100 ns intervals.
    Some((ours as f64 - theirs as f64) / 10_000_000.0)
}

/// True when running from an MSIX package (Microsoft Store install). Packaged processes
/// get virtualized HKCU writes, so the Run-key autostart is replaced by the manifest's
/// `windows.startupTask`, which users control under Settings > Apps > Startup.
pub fn is_packaged() -> bool {
    const APPMODEL_ERROR_NO_PACKAGE: i32 = 15700;
    windows_core::link!("kernel32.dll" "system" fn GetCurrentPackageFullName(length: *mut u32, name: windows_core::PWSTR) -> i32);
    let mut len = 0u32;
    // SAFETY: querying the required length only; a null buffer is documented as valid.
    let status = unsafe { GetCurrentPackageFullName(&mut len, PWSTR::null()) };
    status != APPMODEL_ERROR_NO_PACKAGE
}

/// Asks Windows to start this program again (without arguments) after it closed for an update:
/// a Store package update or an installer using Restart Manager. Not after a crash, a hang or
/// a reboot, where the watchdog and the startup entry take care of it.
pub fn register_restart_after_update() -> Result<()> {
    const RESTART_NO_CRASH: u32 = 1;
    const RESTART_NO_HANG: u32 = 2;
    const RESTART_NO_REBOOT: u32 = 8;
    windows_core::link!("kernel32.dll" "system" fn RegisterApplicationRestart(pwzcommandline: PCWSTR, dwflags: u32) -> windows_core::HRESULT);
    // SAFETY: a null command line is documented as valid (restart without arguments).
    unsafe {
        RegisterApplicationRestart(
            PCWSTR::null(),
            RESTART_NO_CRASH | RESTART_NO_HANG | RESTART_NO_REBOOT,
        )
    }
    .ok()
}

/// Starts `exe` with `args` as a detached process (no console window). Returns its pid.
pub fn spawn_detached(exe: &str, args: &[&str]) -> Result<u32> {
    spawn_detached_with(exe, args, 0, None)
}

/// [`spawn_detached`], but the child is not part of the caller's job object or process tree.
///
/// A crash watchdog that dies together with the app it watches is useless: the app is gone, the
/// marker still says the desktop icons are hidden, and nobody restores them. Two things make a
/// kill of the app's job miss the watchdog:
///
/// - `CREATE_BREAKAWAY_FROM_JOB`, so a job with `KILL_ON_JOB_CLOSE` does not take it down
///   (Windows refuses breakaway when the job does not allow it, hence the fallbacks);
/// - `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` pointing at Explorer, so tools that kill a *process
///   tree* (`taskkill /T`, a debugger, a CI runner) do not find it under us either.
///
/// Falls back to an Explorer parenthand over a plain detached spawn and finally the plain one,
/// so a restricted token still leaves a watchdog for the ordinary crash case.
pub fn spawn_detached_outside_job(exe: &str, args: &[&str]) -> Result<u32> {
    let explorer = crate::desktop::window_pid(crate::desktop::shell_window());
    let parent = (explorer != 0).then_some(explorer);
    let mut last: Option<Error> = None;
    for (flags, parent) in [(CREATE_BREAKAWAY_FROM_JOB, parent), (0, parent), (0, None)] {
        match spawn_detached_with(exe, args, flags, parent) {
            Ok(pid) => return Ok(pid),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(Error::from_thread))
}

fn spawn_detached_with(
    exe: &str,
    args: &[&str],
    extra_flags: u32,
    parent: Option<u32>,
) -> Result<u32> {
    let mut cmd = format!("\"{exe}\"");
    for a in args {
        cmd.push(' ');
        cmd.push('"');
        cmd.push_str(a);
        cmd.push('"');
    }
    let mut cmd_w = to_wide(&cmd);
    let exe_w = to_wide(exe);
    let si = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let flags = CREATE_NO_WINDOW as u32 | extra_flags;
    let Some(parent) = parent else {
        // SAFETY: buffers outlive the call; handles are closed afterwards.
        unsafe {
            let ok = CreateProcessW(
                PCWSTR(exe_w.as_ptr()),
                Some(PWSTR(cmd_w.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                None,
                PCWSTR::null(),
                &si,
                &mut pi,
            );
            if !ok.as_bool() {
                return Err(Error::from_thread());
            }
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
            return Ok(pi.dwProcessId);
        }
    };
    spawn_under(parent, &exe_w, &mut cmd_w, flags, &si, &mut pi)
}

/// `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` set to `parent`, so the child is
/// not part of our process tree. The attribute list has to be built around the call.
fn spawn_under(
    parent: u32,
    exe_w: &[u16],
    cmd_w: &mut Vec<u16>,
    flags: u32,
    plain: &STARTUPINFOW,
    pi: &mut PROCESS_INFORMATION,
) -> Result<u32> {
    // SAFETY: the buffers outlive every call below; the attribute list is freed before return.
    unsafe {
        windows_core::link!("kernel32.dll" "system" fn InitializeProcThreadAttributeList(list: *mut core::ffi::c_void, count: u32, flags: u32, size: *mut usize) -> windows_core::BOOL);
        windows_core::link!("kernel32.dll" "system" fn UpdateProcThreadAttribute(list: *mut core::ffi::c_void, flags: u32, attribute: usize, value: *const core::ffi::c_void, size: usize, previous: *mut core::ffi::c_void, returned: *mut usize) -> windows_core::BOOL);
        windows_core::link!("kernel32.dll" "system" fn DeleteProcThreadAttributeList(list: *mut core::ffi::c_void));
        const PROC_THREAD_ATTRIBUTE_PARENT_PROCESS: usize = 0x0002_0000;
        const PROCESS_CREATE_PROCESS: u32 = 0x0080;
        const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;

        let process = OpenProcess(PROCESS_CREATE_PROCESS, false, parent);
        if process.0.is_null() {
            return Err(Error::from_thread());
        }
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr() as *mut core::ffi::c_void;
        let mut result = Err(Error::from_thread());
        if size > 0 && InitializeProcThreadAttributeList(list, 1, 0, &mut size).as_bool() {
            let updated = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PARENT_PROCESS,
                &process as *const HANDLE as *const core::ffi::c_void,
                size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
            .as_bool();
            if updated {
                let si = StartupInfoEx {
                    info: STARTUPINFOW {
                        cb: size_of::<StartupInfoEx>() as u32,
                        ..*plain
                    },
                    attributes: list,
                };
                let ok = CreateProcessW(
                    PCWSTR(exe_w.as_ptr()),
                    Some(PWSTR(cmd_w.as_mut_ptr())),
                    None,
                    None,
                    false,
                    flags | EXTENDED_STARTUPINFO_PRESENT,
                    None,
                    PCWSTR::null(),
                    &si.info,
                    pi,
                );
                result = if ok.as_bool() {
                    let _ = CloseHandle(pi.hThread);
                    let _ = CloseHandle(pi.hProcess);
                    Ok(pi.dwProcessId)
                } else {
                    Err(Error::from_thread())
                };
            }
            DeleteProcThreadAttributeList(list);
        }
        let _ = CloseHandle(process);
        result
    }
}

/// `STARTUPINFOEXW`: `STARTUPINFOW` followed by the attribute list pointer.
#[repr(C)]
struct StartupInfoEx {
    info: STARTUPINFOW,
    attributes: *mut core::ffi::c_void,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both spawn paths must really start something; breakaway only changes which job the child
    /// belongs to, and a job that forbids it must not stop the watchdog from starting at all.
    #[test]
    fn detached_spawns_start_a_process() {
        let cmd = r"C:\Windows\System32\cmd.exe";
        for pid in [
            spawn_detached(cmd, &["/c", "exit", "0"]).expect("plain spawn"),
            spawn_detached_outside_job(cmd, &["/c", "exit", "0"]).expect("breakaway spawn"),
        ] {
            assert!(pid > 0);
        }
    }
}

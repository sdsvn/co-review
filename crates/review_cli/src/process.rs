//! Other processes by id: whether one runs, and stopping one.

#[cfg(unix)]
pub fn is_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists and may be signalled.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(unix)]
fn signal(pid: u32, signal: i32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: sends a signal to one process id; no memory is involved.
    unsafe { libc::kill(pid, signal) == 0 }
}

/// Asks the process to quit (SIGTERM); false when it could not be signalled.
#[cfg(unix)]
pub fn terminate(pid: u32) -> bool {
    signal(pid, libc::SIGTERM)
}

#[cfg(unix)]
pub fn kill(pid: u32) -> bool {
    signal(pid, libc::SIGKILL)
}

#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(windows)]
pub fn terminate(pid: u32) -> bool {
    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
pub fn kill(pid: u32) -> bool {
    std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}

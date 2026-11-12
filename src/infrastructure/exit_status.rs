use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

/// `status 128` for an exit code, else the platform's text for the signal.
pub(crate) fn describe(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("status {code}"),
        None => status.to_string(),
    }
}

/// Interpret the completed child only; signal and terminal ownership stay native.
pub(crate) fn is_cancelled(status: ExitStatus) -> bool {
    status.code() == Some(130) || status.signal() == Some(rustix::process::Signal::INT.as_raw())
}

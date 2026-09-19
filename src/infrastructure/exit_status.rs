use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

/// Interpret the completed child only; signal and terminal ownership stay native.
pub(crate) fn is_cancelled(status: ExitStatus) -> bool {
    status.code() == Some(130) || status.signal() == Some(rustix::process::Signal::INT.as_raw())
}

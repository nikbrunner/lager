#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(unix)]
static INTERRUPTED: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn record_interrupt(signal: libc::c_int) {
    let _ = INTERRUPTED.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

pub(super) struct InventorySignals {
    #[cfg(unix)]
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

impl InventorySignals {
    pub(super) fn install() -> std::io::Result<Self> {
        #[cfg(unix)]
        let mut guard = Self {
            previous: Vec::new(),
        };
        #[cfg(not(unix))]
        let guard = Self {};
        #[cfg(unix)]
        {
            INTERRUPTED.store(0, Ordering::Relaxed);
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: both actions are initialized C structs. The handler only records a
                // lock-free atomic; cancellation and terminal restoration run on the main thread.
                unsafe {
                    let mut action: libc::sigaction = std::mem::zeroed();
                    let mut previous: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = record_interrupt as *const () as libc::sighandler_t;
                    action.sa_flags = libc::SA_RESTART;
                    libc::sigemptyset(&mut action.sa_mask);
                    if libc::sigaction(signal, &action, &mut previous) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    guard.previous.push((signal, previous));
                }
            }
        }
        Ok(guard)
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            let signal = INTERRUPTED.load(Ordering::Relaxed);
            (signal != 0).then_some(128 + signal)
        }
        #[cfg(not(unix))]
        None
    }
}

impl Drop for InventorySignals {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, previous) in self.previous.iter().rev() {
            // SAFETY: each saved action came from a successful sigaction for this signal.
            unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    extern "C" fn previous_policy(_: libc::c_int) {}

    #[test]
    fn inventory_signal_guard_restores_the_previous_actions() {
        // SAFETY: the test saves and restores its process's original actions before returning.
        unsafe {
            let mut originals = InventorySignals {
                previous: Vec::new(),
            };
            for signal in [libc::SIGINT, libc::SIGTERM] {
                let mut ignored: libc::sigaction = std::mem::zeroed();
                let mut original: libc::sigaction = std::mem::zeroed();
                ignored.sa_sigaction = if signal == libc::SIGINT {
                    libc::SIG_IGN
                } else {
                    previous_policy as *const () as libc::sighandler_t
                };
                ignored.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut ignored.sa_mask);
                libc::sigaddset(&mut ignored.sa_mask, libc::SIGUSR1);
                assert_eq!(libc::sigaction(signal, &ignored, &mut original), 0);
                originals.previous.push((signal, original));
            }
            let guard = InventorySignals::install().unwrap();
            assert_eq!(guard.exit_code(), None);
            record_interrupt(libc::SIGTERM);
            assert_eq!(guard.exit_code(), Some(143));
            drop(guard);
            for signal in [libc::SIGINT, libc::SIGTERM] {
                let mut restored: libc::sigaction = std::mem::zeroed();
                assert_eq!(libc::sigaction(signal, std::ptr::null(), &mut restored), 0);
                assert_eq!(
                    restored.sa_sigaction,
                    if signal == libc::SIGINT {
                        libc::SIG_IGN
                    } else {
                        previous_policy as *const () as libc::sighandler_t
                    }
                );
                assert_ne!(restored.sa_flags & libc::SA_RESTART, 0);
                assert_eq!(libc::sigismember(&restored.sa_mask, libc::SIGUSR1), 1);
            }
        }
    }
}

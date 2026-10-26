//! Keep the supervised MCP chain bounded when its original client disappears.
//! A separate process group also collects launcher descendants on cancellation.

use std::io;
use std::process::{Child, Command, ExitStatus};

#[cfg(unix)]
fn parent_pid() -> u32 {
    // SAFETY: getppid has no preconditions or memory arguments.
    unsafe { libc::getppid() as u32 }
}

pub(super) fn prepare(command: &mut Command) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        Some(parent_pid())
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        None
    }
}

pub(super) fn wait(child: Child, parent: Option<u32>) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        use std::time::{Duration, Instant};

        struct OwnedGroup(Child);
        impl OwnedGroup {
            fn signal(&self, signal: i32) {
                // SAFETY: the child was spawned as leader of a new process
                // group; negative pid addresses only that owned group.
                unsafe { libc::kill(-(self.0.id() as i32), signal) };
            }
        }
        impl Drop for OwnedGroup {
            fn drop(&mut self) {
                self.signal(libc::SIGKILL);
                let _ = self.0.wait();
            }
        }

        let mut group = OwnedGroup(child);
        let mut deadline = None;
        loop {
            if let Some(status) = group.0.try_wait()? {
                return Ok(status);
            }
            if parent.is_some_and(|pid| pid <= 1 || parent_pid() != pid) {
                let end = deadline.get_or_insert_with(|| {
                    group.signal(libc::SIGTERM);
                    Instant::now() + Duration::from_secs(1)
                });
                if Instant::now() >= *end {
                    group.signal(libc::SIGKILL);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        let mut child = child;
        child.wait()
    }
}

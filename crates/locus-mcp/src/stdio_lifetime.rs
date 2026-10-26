//! Stdio belongs to the launching client, even if another process retains its
//! pipe. Keep blocking frame reads off the dispatch thread so a partial frame
//! cannot prevent noticing that the original parent has disappeared.

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) struct Input {
    frames: Receiver<Result<Option<super::Frame>>>,
    parent: Option<u32>,
    finished: Arc<AtomicBool>,
}

impl Input {
    pub(super) fn start() -> Result<Self> {
        let parent = parent_pid();
        let finished = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let watchdog_finished = Arc::clone(&finished);
        let watchdog_closed = Arc::clone(&closed);
        // Dispatch may block inside a provider or on a full stdout pipe. The
        // lifetime watcher must be independent of BOTH reads and dispatch.
        std::thread::Builder::new()
            .name("mcp-lifetime".into())
            .spawn(move || {
                let mut deadline = None;
                while !watchdog_finished.load(Ordering::Acquire) {
                    let orphaned = parent.is_some_and(|pid| pid <= 1 || parent_pid() != Some(pid));
                    if orphaned || input_hung_up() || watchdog_closed.load(Ordering::Acquire) {
                        let end =
                            deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(1));
                        if Instant::now() >= *end {
                            // Graceful EOF normally returns through run_stdio.
                            // An unresponsive provider/output must not retain
                            // the server indefinitely after the client is gone.
                            super::teardown_stdio_workers(false);
                            std::process::exit(0);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
            .context("start MCP lifetime watcher")?;
        // One queued frame bounds memory and preserves request ordering and
        // responses to requests submitted immediately before a clean EOF.
        let (send, frames) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("mcp-stdin".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                let mut reader = stdin.lock();
                loop {
                    let frame = super::read_frame(&mut reader);
                    if matches!(&frame, Ok(None)) {
                        closed.store(true, Ordering::Release);
                    }
                    let done = !matches!(&frame, Ok(Some(_)));
                    if send.send(frame).is_err() || done {
                        break;
                    }
                }
            })
            .context("start MCP input reader")?;
        Ok(Self {
            frames,
            parent,
            finished,
        })
    }

    pub(super) fn next(&self) -> Result<Option<super::Frame>> {
        loop {
            if let Some(parent) = self.parent {
                if parent <= 1 || parent_pid() != Some(parent) {
                    return Ok(None);
                }
            }
            match self.frames.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => return frame,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
    }
}

#[cfg(unix)]
fn parent_pid() -> Option<u32> {
    // SAFETY: getppid has no preconditions and does not access Rust memory.
    Some(unsafe { libc::getppid() } as u32)
}

#[cfg(unix)]
fn input_hung_up() -> bool {
    // POLLHUP is reported even with events=0 and unread buffered requests.
    // This keeps bounded queue backpressure from concealing a closed pipe.
    let mut descriptor = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: 0,
        revents: 0,
    };
    // SAFETY: descriptor points to one initialized pollfd for this call only.
    (unsafe { libc::poll(&mut descriptor, 1, 0) > 0 })
        && descriptor.revents & (libc::POLLHUP | libc::POLLERR) != 0
}

#[cfg(not(unix))]
fn input_hung_up() -> bool {
    false
}

#[cfg(not(unix))]
fn parent_pid() -> Option<u32> {
    // Windows pipe closure remains authoritative; POSIX reparenting does not
    // have an equivalent getppid contract there.
    None
}

//! Parent-death watchdog for the client-launched server.
//!
//! stdin EOF is the normal shutdown path, but EOF only arrives when the
//! write end was held exclusively by the dying process. The usual launchers
//! hand this server a *shared* stdin — `locus mcp` spawns it with
//! `Stdio::inherit()` and the npm wrapper does the same — so a killed middle
//! process (or any launcher that is not the sole stdin writer) never
//! delivers EOF: the server stays blocked in `read()` as a reparented
//! orphan, still holding the executor grant and its upstream workers. The
//! HTTP transport never reads stdin at all (`locus serve --swarm` launches
//! the broker with stdin null), so an orphaned broker serves forever.
//!
//! Instead of trusting the pipe, anchor on the launching process itself:
//! when it exits, the session it anchored is over and the server exits too.
//! Unix detection uses kqueue `EVFILT_PROC`/`NOTE_EXIT` on macOS
//! (event-driven) and a `getppid()` poll elsewhere — reparenting always
//! changes the observed parent pid, and unlike `PR_SET_PDEATHSIG` a poll
//! cannot fire on a parent's *thread* exit while the process lives.
//! Non-Unix keeps the stdin-EOF behavior unchanged (Job Objects are the
//! launcher's responsibility there).

/// Arm the watchdog for this process. Returns immediately; when the
/// launching process exits, a background thread ends the server.
pub(crate) fn arm_parent_watchdog() {
    #[cfg(unix)]
    imp::arm();
}

#[cfg(unix)]
mod imp {
    use std::process;
    use std::thread;
    use std::time::Duration;

    /// Detection cadence for the portable poll and the kqueue backstop; far
    /// under the two seconds the lifecycle contract grants a killed parent.
    const PARENT_POLL: Duration = Duration::from_millis(100);

    pub(super) fn arm() {
        let ppid = std::os::unix::process::parent_id();
        if ppid <= 1 {
            // Already reparented (launchd/systemd/double-fork daemon): the
            // launching process is gone or there is no supervising process
            // to lose. stdin EOF remains the shutdown path — matching the
            // pre-watchdog behavior exactly.
            eprintln!("locus-mcp: no supervising parent to watch (ppid {ppid})");
            return;
        }
        eprintln!("locus-mcp: parent watchdog armed (ppid {ppid})");
        let spawn = thread::Builder::new()
            .name("locus-parent-watch".into())
            .spawn(move || {
                wait_for_parent_exit(ppid);
                // stderr only — stdout is the MCP protocol channel.
                eprintln!("locus-mcp: parent process {ppid} exited; shutting down");
                process::exit(0);
            });
        if let Err(e) = spawn {
            // A watchdog that cannot start degrades to the previous
            // stdin-EOF semantics; serving with a warning beats refusing.
            eprintln!("locus-mcp: parent watchdog unavailable: {e}");
        }
    }

    /// Block until the recorded parent pid is no longer our parent.
    fn wait_for_parent_exit(ppid: u32) {
        #[cfg(target_os = "macos")]
        if let Some(kq) = ParentExitKqueue::new(ppid) {
            kq.wait();
            return;
        }
        poll_parent_exit(ppid);
    }

    /// Reparenting is the only legitimate ppid change: any difference means
    /// the recorded parent is gone.
    fn poll_parent_exit(ppid: u32) {
        while std::os::unix::process::parent_id() == ppid {
            thread::sleep(PARENT_POLL);
        }
    }

    /// macOS event-driven parent exit: one `EVFILT_PROC` registration on the
    /// pid delivers `NOTE_EXIT` without polling. The bounded `kevent` wait
    /// re-checks `getppid()` so a registration raced with a dying parent —
    /// or any silently missed edge — still converges on the poll path.
    #[cfg(target_os = "macos")]
    struct ParentExitKqueue {
        fd: std::os::fd::RawFd,
        ppid: u32,
    }

    #[cfg(target_os = "macos")]
    impl ParentExitKqueue {
        fn new(ppid: u32) -> Option<Self> {
            // SAFETY: kqueue() has no preconditions.
            let fd = unsafe { libc::kqueue() };
            if fd < 0 {
                return None;
            }
            let change = libc::kevent {
                ident: ppid as usize,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            // SAFETY: `change` is a valid kevent; the event list is empty.
            let n =
                unsafe { libc::kevent(fd, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
            if n < 0 {
                // SAFETY: fd is an open kqueue owned by this scope.
                unsafe { libc::close(fd) };
                return None;
            }
            Some(Self { fd, ppid })
        }

        fn wait(&self) {
            let timeout = libc::timespec {
                tv_sec: 0,
                tv_nsec: PARENT_POLL.as_nanos() as i64,
            };
            loop {
                let mut out = std::mem::MaybeUninit::<libc::kevent>::uninit();
                // SAFETY: `out` is a writable slot for one event; `timeout`
                // is a valid timespec.
                let n = unsafe {
                    libc::kevent(self.fd, std::ptr::null(), 0, out.as_mut_ptr(), 1, &timeout)
                };
                if n > 0 {
                    // SAFETY: kevent filled `out` with one delivered event.
                    let event = unsafe { out.assume_init() };
                    if event.fflags & libc::NOTE_EXIT != 0 {
                        return;
                    }
                }
                // Poll backstop (also re-enters cleanly on EINTR): catches a
                // death that raced registration and any non-exit note.
                if std::os::unix::process::parent_id() != self.ppid {
                    return;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for ParentExitKqueue {
        fn drop(&mut self) {
            // SAFETY: fd is a live kqueue owned by this guard.
            unsafe { libc::close(self.fd) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn poll_returns_immediately_for_a_parent_that_is_not_ours() {
            // `parent_id()` can never equal a bogus pid, so the loop must not
            // take even one sleep.
            let start = std::time::Instant::now();
            poll_parent_exit(0);
            assert!(start.elapsed() < PARENT_POLL);
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn kqueue_registration_fails_closed_to_poll_for_a_dead_pid() {
            // Registering NOTE_EXIT on pid 0 either fails or never fires; the
            // getppid backstop must still return promptly.
            let start = std::time::Instant::now();
            wait_for_parent_exit(0);
            assert!(start.elapsed() < PARENT_POLL * 5);
        }
    }
}

//! A worker lease that survives abrupt owner death. The guardian contains no
//! Rust runtime after fork: only async-signal-safe syscalls, then `_exit`.
//!
//! The worker gets its own process group. Before exec it publishes that group
//! to the guardian; the guardian joins it, reserving the group ID even if the
//! owner dies and init reaps the worker. Closing the lease (or losing the owner)
//! gives stdin EOF a short grace period, then kills the entire remaining group.

use crate::error::{LocusError, Result};
use std::process::{Child, Command};

pub(super) struct GuardedChild {
    pub child: Child,
    terminated: bool,
    #[cfg(unix)]
    lease: Lease,
}

impl GuardedChild {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            let lease = Lease::attach(command)?;
            let child = command.spawn()?;
            Ok(Self {
                child,
                lease,
                terminated: false,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            Err(LocusError::msg(
                "MCP worker parent-death guard unsupported on this platform",
            ))
        }
    }

    pub fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        self.child.stdin.take();
        #[cfg(unix)]
        self.lease.close();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn is_running(&self) -> Result<bool> {
        if self.terminated {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            // Do not reap here: the guardian must finish group cleanup before
            // the worker's PID can be reused.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            loop {
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        self.child.id(),
                        &mut info,
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if result == 0 {
                    return Ok(unsafe { info.si_pid() } == 0);
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.into());
                }
            }
        }
        #[cfg(not(unix))]
        Err(LocusError::msg(
            "MCP worker liveness unsupported on this platform",
        ))
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
struct Lease {
    writer: Option<std::fs::File>,
    guardian: libc::pid_t,
}

#[cfg(unix)]
impl Lease {
    fn attach(command: &mut Command) -> Result<Self> {
        use std::os::fd::{AsRawFd, OwnedFd};
        use std::os::unix::net::UnixStream;
        use std::os::unix::process::CommandExt;
        // std creates these CLOEXEC atomically; an unrelated concurrent exec
        // must not inherit a lease and postpone shutdown.
        let (reader, writer) = UnixStream::pair()?;
        let read_fd = reader.as_raw_fd();
        let owner = unsafe { libc::getpid() };
        let max_fd = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
        if max_fd < 0 || max_fd > libc::c_int::MAX as libc::c_long {
            return Err(LocusError::msg("cannot bound guardian descriptor cleanup"));
        }
        // Block delivery across fork so no inherited application handler can
        // run before the guardian resets dispositions. Only this thread changes.
        let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut original: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigfillset(&mut blocked);
        }
        let mask_error =
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &blocked, &mut original) };
        if mask_error != 0 {
            return Err(std::io::Error::from_raw_os_error(mask_error).into());
        }
        let guardian = unsafe { libc::fork() };
        let fork_error = std::io::Error::last_os_error();
        if guardian != 0 {
            unsafe {
                libc::pthread_sigmask(libc::SIG_SETMASK, &original, std::ptr::null_mut());
            }
        }
        if guardian < 0 {
            return Err(fork_error.into());
        }
        if guardian == 0 {
            // SAFETY: no allocation, destructors, locks, or Rust runtime in
            // this fork branch. The guardian must not retain ANY worker pipes,
            // authority sockets or parent stdio, including sibling workers'.
            unsafe { guard(read_fd, owner, max_fd as libc::c_int) }
        }
        drop(reader);
        // Retain ownership while installing pre_exec so the descriptor stays with the
        // lease even if spawn fails. Only the child writes the PID, once.
        let writer = std::fs::File::from(OwnedFd::from(writer));
        let published_fd = writer.as_raw_fd();
        command.process_group(0);
        unsafe {
            command.pre_exec(move || {
                let pid = libc::getpid();
                loop {
                    let n = libc::write(
                        published_fd,
                        (&pid as *const libc::pid_t).cast(),
                        std::mem::size_of_val(&pid),
                    );
                    if n == std::mem::size_of_val(&pid) as isize {
                        let mut ready = libc::pollfd {
                            fd: published_fd,
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        let mut ack = 0u8;
                        for _ in 0..100 {
                            let available = libc::poll(&mut ready, 1, 100);
                            if available > 0 {
                                let received =
                                    libc::read(published_fd, (&mut ack as *mut u8).cast(), 1);
                                if received == 1 && ack == 1 {
                                    return Ok(());
                                }
                                if received == 0 {
                                    break;
                                }
                                if received < 0
                                    && std::io::Error::last_os_error().raw_os_error()
                                        == Some(libc::EINTR)
                                {
                                    continue;
                                }
                                break;
                            }
                            if available < 0
                                && std::io::Error::last_os_error().raw_os_error()
                                    != Some(libc::EINTR)
                            {
                                break;
                            }
                        }
                        return Err(std::io::Error::from_raw_os_error(libc::ETIMEDOUT));
                    }
                    let error = std::io::Error::last_os_error();
                    if n >= 0 || error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(std::io::Error::from_raw_os_error(libc::EIO));
                    }
                }
            });
        }
        Ok(Self {
            writer: Some(writer),
            guardian,
        })
    }

    fn close(&mut self) {
        if self.writer.take().is_none() {
            return;
        }
        loop {
            let result = unsafe { libc::waitpid(self.guardian, std::ptr::null_mut(), 0) };
            if result >= 0
                || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
    }
}

#[cfg(unix)]
impl Drop for Lease {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(unix)]
unsafe fn guard(read_fd: libc::c_int, owner: libc::pid_t, max_fd: libc::c_int) -> ! {
    // Never run an inherited application signal handler in a forked guardian.
    // Group-directed termination must not disable cleanup of stubborn workers.
    for signal in 1..=64 {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = if matches!(
            signal,
            libc::SIGTERM | libc::SIGINT | libc::SIGHUP | libc::SIGQUIT | libc::SIGPIPE
        ) {
            libc::SIG_IGN
        } else {
            libc::SIG_DFL
        };
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(signal, &action, std::ptr::null_mut());
    }
    let mut mask: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut mask);
    libc::sigprocmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut());
    // Reserve fd 3, then discard all other inherited descriptors. Linux has a
    // fast syscall; the portable fallback also works on macOS without libproc.
    if read_fd != 3 && libc::dup2(read_fd, 3) < 0 {
        libc::_exit(1);
    }
    for fd in 0..3 {
        libc::close(fd);
    }
    #[cfg(target_os = "linux")]
    let closed = libc::syscall(libc::SYS_close_range, 4u32, u32::MAX, 0u32) == 0;
    #[cfg(not(target_os = "linux"))]
    let closed = false;
    if !closed {
        for fd in 4..max_fd {
            libc::close(fd);
        }
    }
    let mut pid: libc::pid_t = 0;
    let bytes = std::mem::size_of_val(&pid);
    let mut received = 0;
    while received < bytes {
        // If the owner dies before registration, the pending child cannot
        // receive an ACK or exec. Do not wait forever on another fork's copy
        // of the lease writer in this startup window.
        if libc::getppid() != owner {
            libc::_exit(0);
        }
        let mut pending = libc::pollfd {
            fd: 3,
            events: libc::POLLIN,
            revents: 0,
        };
        let available = libc::poll(&mut pending, 1, 100);
        if available == 0 {
            continue;
        }
        if available < 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            libc::_exit(1);
        }
        let n = libc::read(
            3,
            (&mut pid as *mut libc::pid_t)
                .cast::<u8>()
                .add(received)
                .cast(),
            bytes - received,
        );
        if n > 0 {
            received += n as usize;
        } else if n == 0 {
            libc::_exit(0);
        } else if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            libc::_exit(1);
        }
    }
    if pid <= 1 || libc::setpgid(0, pid) != 0 {
        libc::_exit(1);
    }
    let ack = 1u8;
    loop {
        if libc::write(3, (&ack as *const u8).cast(), 1) == 1 {
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            libc::_exit(1);
        }
    }
    let mut fd = libc::pollfd {
        fd: 3,
        events: libc::POLLIN,
        revents: 0,
    };
    while libc::getppid() == owner {
        let result = libc::poll(&mut fd, 1, 100);
        if result > 0 {
            break;
        }
        if result < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    // Parent death already closed the only protocol writer; ordinary teardown
    // closes it before the lease. Let well-behaved servers observe EOF first.
    let mut delay = libc::timespec {
        tv_sec: 0,
        tv_nsec: 250_000_000,
    };
    while libc::nanosleep(&delay, &mut delay) != 0 {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            break;
        }
    }
    libc::kill(-pid, libc::SIGKILL); // includes this guardian: reserves PGID until kill
    libc::_exit(0);
}

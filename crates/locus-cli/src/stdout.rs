//! Broken-pipe-safe stdout for the `locus` CLI.
//!
//! `std::println!` panics with "failed printing to stdout" when the reader
//! goes away (`locus doctor | head -1`). Resetting SIGPIPE to its default
//! would fix that, but `locus` processes also host the authority broker
//! (Unix-socket writes) and supervised MCP workers (child-stdin writes): with
//! SIG_DFL a peer that disconnects early would kill those silently instead of
//! returning `EPIPE`. So SIGPIPE stays ignored and only stdout is handled.
//!
//! The crate-local `println!` / `print!` below shadow the std macros for every
//! module declared after this one. On `BrokenPipe` they unwind with
//! [`StdoutClosed`] via `resume_unwind` (no panic hook, so nothing is printed,
//! and destructors such as worker teardown still run); `main` maps that to
//! `ExitCode::StdoutClosed` (141 = 128 + SIGPIPE), the shell convention, which stays non-zero
//! so a truncated report never reads as success. Any other stdout error keeps
//! std's panic.

use std::io::{self, Write};

/// Unwind payload for "stdout reader went away".
pub(crate) struct StdoutClosed;

/// Write formatted output to stdout; a closed pipe unwinds with [`StdoutClosed`].
pub(crate) fn write_stdout(args: std::fmt::Arguments<'_>) {
    let mut out = io::stdout().lock();
    if let Err(e) = out.write_fmt(args) {
        on_stdout_error(e);
    }
}

/// Write raw bytes to stdout (e.g. generated completions) with the same handling.
pub(crate) fn write_stdout_bytes(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    if let Err(e) = out.write_all(bytes).and_then(|()| out.flush()) {
        on_stdout_error(e);
    }
}

fn on_stdout_error(e: io::Error) -> ! {
    if e.kind() == io::ErrorKind::BrokenPipe {
        std::panic::resume_unwind(Box::new(StdoutClosed));
    }
    panic!("failed printing to stdout: {e}");
}

/// Run `f`, turning a [`StdoutClosed`] unwind into `Err(())`; other panics resume.
pub(crate) fn catch_stdout_closed<T>(f: impl FnOnce() -> T) -> Result<T, ()> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) if payload.is::<StdoutClosed>() => Err(()),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

macro_rules! println {
    () => {
        $crate::stdout::write_stdout(::std::format_args!("\n"))
    };
    ($($arg:tt)*) => {
        $crate::stdout::write_stdout(::std::format_args!(
            "{}\n",
            ::std::format_args!($($arg)*)
        ))
    };
}

macro_rules! print {
    ($($arg:tt)*) => {
        $crate::stdout::write_stdout(::std::format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_stdout_unwind_is_caught_without_panic_hook() {
        let r: Result<(), ()> =
            catch_stdout_closed(|| std::panic::resume_unwind(Box::new(StdoutClosed)));
        assert_eq!(r, Err(()));
    }

    #[test]
    fn other_panics_still_propagate() {
        let r = std::panic::catch_unwind(|| {
            let _ = catch_stdout_closed(|| panic!("real bug"));
        });
        assert!(r.is_err());
    }

    #[test]
    fn non_broken_pipe_errors_keep_std_panic() {
        let r = std::panic::catch_unwind(|| {
            on_stdout_error(io::Error::other("disk full"));
        });
        let payload = r.unwrap_err();
        assert!(!payload.is::<StdoutClosed>());
    }

    #[test]
    fn broken_pipe_unwinds_with_marker() {
        let r = std::panic::catch_unwind(|| {
            on_stdout_error(io::Error::from(io::ErrorKind::BrokenPipe));
        });
        assert!(r.unwrap_err().is::<StdoutClosed>());
    }
}

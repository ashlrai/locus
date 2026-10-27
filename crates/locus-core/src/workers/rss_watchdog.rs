//! Child-owned RSS supervision. Diagnostics never include child output or identity.
use crate::error::{LocusError, Result};
use std::process::Child;
#[cfg(any(not(any(target_os = "linux", target_os = "macos")), test))]
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) const DEFAULT_CAP: u64 = 1024 * 1024 * 1024;
const INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct WatchedChild {
    child: Arc<Mutex<Child>>,
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
    exceeded: Arc<std::sync::atomic::AtomicBool>,
}

impl WatchedChild {
    pub(crate) fn start(child: Child, cap: u64) -> Result<Self> {
        Self::with_interval(child, cap, INTERVAL)
    }

    fn with_interval(child: Child, cap: u64, interval: Duration) -> Result<Self> {
        let child = Arc::new(Mutex::new(child));
        let exceeded = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (stop, receiver) = mpsc::channel();
        let watched = child.clone();
        let flag = exceeded.clone();
        let thread = std::thread::Builder::new()
            .name("locus-rss".into())
            .spawn(move || {
                loop {
                    let mut child = watched.lock().unwrap_or_else(|e| e.into_inner());
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    match resident_bytes(child.id()) {
                        Ok(rss) if rss > cap => {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                            eprintln!("locus: child_rss_cap_exceeded");
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        // An unenforceable limit must not silently become unlimited.
                        Err(_) => {
                            flag.store(true, std::sync::atomic::Ordering::Release);
                            eprintln!("locus: child_rss_sample_failed");
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        _ => {}
                    }
                    drop(child);
                    if receiver.recv_timeout(interval) != Err(mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            });
        match thread {
            Ok(thread) => Ok(Self {
                child,
                stop,
                thread: Some(thread),
                exceeded,
            }),
            Err(_) => {
                let mut child = child.lock().unwrap_or_else(|e| e.into_inner());
                let _ = child.kill();
                let _ = child.wait();
                Err(LocusError::msg("child RSS watchdog unavailable"))
            }
        }
    }

    pub(crate) fn exceeded(&self) -> bool {
        self.exceeded.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl Drop for WatchedChild {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn resident_bytes(pid: u32) -> std::io::Result<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
        let kb = status
            .lines()
            .find_map(|line| line.strip_prefix("VmRSS:"))
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok());
        kb.and_then(|kb| kb.checked_mul(1024))
            .ok_or_else(|| std::io::Error::other("RSS unavailable"))
    }
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
        // SAFETY: proc_pidinfo receives a correctly sized, writable taskinfo buffer;
        // it is read only when the kernel reports a complete structure.
        let size = std::mem::size_of::<libc::proc_taskinfo>();
        let read = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size as libc::c_int,
            )
        };
        if read != size as libc::c_int {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { info.assume_init() }.pti_resident_size)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Only RSS for this PID, never command arguments or environment.
        let output = Command::new("/bin/ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        let bytes = std::str::from_utf8(&output.stdout)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .and_then(|kb| kb.checked_mul(1024));
        bytes
            .filter(|_| output.status.success())
            .ok_or_else(|| std::io::Error::other("RSS unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_is_one_gib() {
        assert_eq!(DEFAULT_CAP, 1 << 30);
    }

    #[cfg(unix)]
    #[test]
    fn allocating_child_is_killed_and_reaped() {
        // Use this test executable as the fake child; no interpreter dependency.
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "workers::rss_watchdog::tests::fake_allocator"])
            .env("LOCUS_FAKE_ALLOCATOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(
            resident_bytes(child.id()).is_ok(),
            "RSS sampling unavailable"
        );
        let watched =
            WatchedChild::with_interval(child, 16 * 1024 * 1024, Duration::from_millis(20))
                .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !watched.exceeded() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(watched.exceeded());
        assert!(!watched.child.lock().unwrap().wait().unwrap().success());
    }

    #[cfg(unix)]
    #[test]
    fn healthy_child_survives_and_drop_stops_and_reaps_promptly() {
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(resident_bytes(child.id()).unwrap() > 0);
        let watched = WatchedChild::start(child, u64::MAX).unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(!watched.exceeded());
        let child = watched.child.clone();
        assert!(child.lock().unwrap().try_wait().unwrap().is_none());
        let start = std::time::Instant::now();
        drop(watched);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(child.lock().unwrap().try_wait().unwrap().is_some());
    }

    #[test]
    fn fake_allocator() {
        if std::env::var("LOCUS_FAKE_ALLOCATOR").as_deref() != Ok("1") {
            return;
        }
        let memory = vec![42u8; 64 * 1024 * 1024];
        std::hint::black_box(&memory);
        std::thread::sleep(Duration::from_secs(30));
        std::hint::black_box(memory);
    }
}

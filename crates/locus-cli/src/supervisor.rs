//! A durable parent for the existing authority broker. This module cannot
//! issue, validate, or resurrect authority: all of that stays in locus-core.
//! The helper is essential: the broker captures its launcher's *parent*, so
//! calling the store directly in the daemon would still bind it to the shell.

use anyhow::{bail, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum SupervisorCmd {
    /// Detach a long-lived supervisor; then use normal `locus enter`
    Start,
    /// Stay in the foreground (for a service manager); then use normal `locus enter`
    Run,
}

pub(crate) fn run(command: SupervisorCmd, json: bool) -> Result<()> {
    #[cfg(unix)]
    return unix::run(command, json);
    #[cfg(not(unix))]
    {
        let _ = (command, json);
        bail!("durable supervision requires Unix process identity support")
    }
}

pub(crate) fn run_internal_if_requested() -> Option<Result<()>> {
    let mode = std::env::var_os("LOCUS_INTERNAL_SUPERVISOR")?;
    #[cfg(unix)]
    return Some(unix::internal(&mode));
    #[cfg(not(unix))]
    {
        let _ = mode;
        Some(Err(anyhow::anyhow!("durable supervision requires Unix")))
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::{ensure, Context};
    use locus_core::{Store, CONTROL_CAPABILITY_ENV, EXECUTOR_CAPABILITY_ENV};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::ffi::OsStr;
    use std::fs::{self, File, OpenOptions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use zeroize::{Zeroize, Zeroizing};

    const INTERNAL: &str = "LOCUS_INTERNAL_SUPERVISOR";
    const MAX_BYTES: u64 = 16 * 1024;
    const START_TIMEOUT: Duration = Duration::from_secs(45);
    const POLL: Duration = Duration::from_millis(250);

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Startup {
        home: PathBuf,
        capability: String,
    }

    impl Drop for Startup {
        fn drop(&mut self) {
            self.capability.zeroize();
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Ready {
        supervisor_pid: u32,
        broker_pid: u32,
        epoch: String,
    }

    struct PendingChild {
        child: Child,
        handed_off: bool,
    }

    impl Drop for PendingChild {
        fn drop(&mut self) {
            if !self.handed_off {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    pub(super) fn run(command: SupervisorCmd, json: bool) -> Result<()> {
        // Never strip a confinement signal and turn an executor into a control
        // launcher. Only the operator's explicitly supplied capability is used.
        ensure!(
            std::env::var_os(EXECUTOR_CAPABILITY_ENV).is_none()
                && std::env::var_os("LOCUS_SESSION_ID").is_none(),
            "supervisor must be started outside a delegated session by the human operator"
        );
        let capability = Zeroizing::new(std::env::var(CONTROL_CAPABILITY_ENV).map_err(|_| {
            anyhow::anyhow!(
                "a valid LOCUS_CONTROL_CAPABILITY is required; human: load the Locus shell hook"
            )
        })?);
        validate_capability(&capability)?;
        let store = Store::open_default()?;
        let startup = Startup {
            home: fs::canonicalize(store.home())?,
            capability: capability.to_string(),
        };
        drop(capability);
        match command {
            SupervisorCmd::Start => {
                let mut child = PendingChild {
                    child: spawn("daemon", &startup, true)?,
                    handed_off: false,
                };
                let ready: Ready = receive(&mut child.child)?;
                display(&ready, json)?;
                child.handed_off = true;
                // Child is intentionally detached. It owns no caller streams
                // and lives only while the exact broker generation remains.
                Ok(())
            }
            SupervisorCmd::Run => {
                // This process becomes the long-lived owner. Retain none of
                // the operator's provider credentials or control environment.
                for (name, _) in std::env::vars_os() {
                    std::env::remove_var(name);
                }
                std::env::set_current_dir(&startup.home)?;
                supervise(startup, |ready| display(ready, json))
            }
        }
    }

    pub(super) fn internal(mode: &OsStr) -> Result<()> {
        let mut raw = Zeroizing::new(String::new());
        BufReader::new(std::io::stdin().take(MAX_BYTES + 1)).read_line(&mut raw)?;
        ensure!(
            raw.len() as u64 <= MAX_BYTES,
            "supervisor startup is too large"
        );
        // Never include the input or serde's offending value in diagnostics.
        let startup: Startup = serde_json::from_str(&raw)
            .map_err(|_| anyhow::anyhow!("invalid supervisor startup"))?;
        validate_capability(&startup.capability)?;
        drop(raw);
        ensure!(
            std::env::var_os(EXECUTOR_CAPABILITY_ENV).is_none()
                && std::env::var_os("LOCUS_SESSION_ID").is_none(),
            "supervisor cannot run inside a delegated session"
        );
        match mode.to_str() {
            Some("daemon") => supervise(startup, write_json),
            Some("bootstrap") => {
                std::env::set_var(CONTROL_CAPABILITY_ENV, &startup.capability);
                let store = Store::open_existing(&startup.home)?;
                let posture = locus_core::control_capability_status(store.home());
                ensure!(
                    !posture.persisted
                        || (posture.persisted_valid
                            && posture.persisted_permissions_ok
                            && posture.matches_persisted == Some(true)),
                    "supervisor requires the matching persisted operator capability"
                );
                // Same control/seal gates as enter. This only starts/authenticates
                // a broker; it neither rewrites active.json nor issues a pin.
                store.require_local_control_to_replace_active("locus supervisor start")?;
                std::env::remove_var(CONTROL_CAPABILITY_ENV);
                write_json(&endpoint(&startup.home)?)
            }
            _ => bail!("unknown internal supervisor mode"),
        }
    }

    fn validate_capability(value: &str) -> Result<()> {
        ensure!(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid control capability (expected 64 lowercase hex characters)"
        );
        Ok(())
    }

    fn spawn(mode: &str, startup: &Startup, detach: bool) -> Result<Child> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .env_clear()
            .env(INTERNAL, mode)
            .current_dir(&startup.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if detach {
            // SAFETY: setsid is async-signal-safe; no allocation or locks run
            // between fork and exec. No inherited terminal/process group owns
            // the daemon, so closing the entering shell cannot SIGHUP it.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = command.spawn().context("start supervisor process")?;
        let sent = (|| -> Result<()> {
            let mut input = child
                .stdin
                .take()
                .context("supervisor startup pipe unavailable")?;
            serde_json::to_writer(&mut input, startup)?;
            input.write_all(b"\n")?;
            Ok(())
        })();
        if let Err(error) = sent {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(child)
    }

    /// Bounded handoff with cleanup of the *owned child*, never a PID read
    /// from disk. No secret is returned through stdout or stderr.
    fn receive<T: for<'a> Deserialize<'a>>(child: &mut Child) -> Result<T> {
        let stdout = child
            .stdout
            .take()
            .context("supervisor handoff unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("supervisor diagnostics unavailable")?;
        let (errors_tx, errors_rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut error = String::new();
            let _ = stderr.take(MAX_BYTES).read_to_string(&mut error);
            let _ = errors_tx.send(error);
        });
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout.take(MAX_BYTES))
                .read_line(&mut line)
                .map(|_| line);
            let _ = tx.send(result);
        });
        let result = match rx.recv_timeout(START_TIMEOUT) {
            Ok(Ok(line)) => serde_json::from_str(&line)
                .map_err(|_| anyhow::anyhow!("supervisor did not become ready")),
            _ => Err(anyhow::anyhow!(
                "supervisor startup timed out or pipe failed"
            )),
        };
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            if let Ok(error) = errors_rx.recv_timeout(Duration::from_secs(1)) {
                if !error.trim().is_empty() {
                    return result.context(error.trim().to_string());
                }
            }
        }
        result
    }

    fn supervise(startup: Startup, ready: impl FnOnce(&Ready) -> Result<()>) -> Result<()> {
        let mut helper = PendingChild {
            child: spawn("bootstrap", &startup, false)?,
            handed_off: false,
        };
        let initial: Value = receive(&mut helper.child)?;
        ensure!(
            helper.child.wait()?.success(),
            "supervisor bootstrap failed"
        );
        ensure!(
            initial["supervisor"]["pid"].as_u64() == Some(u64::from(std::process::id())),
            "an existing broker belongs to another supervisor; keep using it, or stop its owning shell/service before starting a durable supervisor"
        );
        let broker_pid = initial["owner_pid"]
            .as_u64()
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32 && *pid != std::process::id())
            .context("invalid supervised broker PID")?;
        let epoch = initial["epoch"]
            .as_str()
            .context("broker epoch missing")?
            .to_string();
        let home = startup.home.clone();
        // Authority remains only in the broker; the idle supervisor needs no
        // bearer, binding, credential provider, or environment from the caller.
        drop(startup);
        ready(&Ready {
            supervisor_pid: std::process::id(),
            broker_pid,
            epoch,
        })?;
        loop {
            thread::sleep(POLL);
            // Metadata is used only to end this process, never to authorize or
            // connect to a provider. The broker independently checks this exact
            // process's PID/start time/executable and the session generation.
            if endpoint(&home).ok().as_ref() != Some(&initial) || !process_exists(broker_pid) {
                return Ok(());
            }
        }
    }

    fn process_exists(pid: u32) -> bool {
        // SAFETY: signal 0 checks existence; it never signals a stored PID.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    fn endpoint(home: &Path) -> Result<Value> {
        let file: File = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(home.join("runtime/authority.json"))?;
        let metadata = file.metadata()?;
        // SAFETY: geteuid has no preconditions.
        ensure!(
            metadata.is_file()
                && metadata.nlink() == 1
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.len() <= MAX_BYTES,
            "unsafe broker metadata"
        );
        Ok(serde_json::from_reader(file.take(MAX_BYTES))?)
    }

    fn write_json(value: &impl Serialize) -> Result<()> {
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        serde_json::to_writer(&mut output, value)?;
        output.write_all(b"\n")?;
        output.flush()?;
        Ok(())
    }

    fn display(ready: &Ready, json: bool) -> Result<()> {
        if json {
            crate::json_out::print(
                crate::json_out::SUPERVISOR,
                &serde_json::json!({
                    "ready": true,
                    "supervisor_pid": ready.supervisor_pid,
                    "broker_pid": ready.broker_pid,
                    "epoch": ready.epoch,
                }),
            )
        } else {
            println!(
                "Supervisor ready (pid {}, broker {}). Use `locus enter <alias>` to pin.",
                ready.supervisor_pid, ready.broker_pid
            );
            Ok(())
        }
    }
}

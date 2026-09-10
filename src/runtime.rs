use crate::{
    current_hint, formatter::Formatter, indented_workspaces, labels, saved_workspaces, text_field,
    token_text, SOURCE,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

fn output(command: &mut Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut stdout = child.stdout.take().ok_or("missing subprocess stdout")?;
        let mut stderr = child.stderr.take().ok_or("missing subprocess stderr")?;
        nonblocking(&stdout).map_err(|e| e.to_string())?;
        nonblocking(&stderr).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut out_closed, mut err_closed) = (false, false);
        let status = loop {
            if !out_closed {
                out_closed = drain(&mut stdout, &mut out).map_err(|e| e.to_string())?;
            }
            if !err_closed {
                err_closed = drain(&mut stderr, &mut err).map_err(|e| e.to_string())?;
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if out_closed && err_closed {
                    break status;
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        Ok(Output {
            success: status.success(),
            stdout: String::from_utf8(out).map_err(|e| e.to_string())?,
            stderr: String::from_utf8(err).map_err(|e| e.to_string())?,
        })
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn nonblocking(pipe: &impl AsRawFd) -> std::io::Result<()> {
    // The pipe owns this valid descriptor for the entire call.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    let result = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>) -> std::io::Result<bool> {
    let mut buffer = [0; 8192];
    // Bound each pass so continuous output cannot postpone the deadline.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub fn branch_at(path: Option<&str>) -> Result<Option<String>, String> {
    let Some(path) = path.filter(|p| Path::new(p).is_absolute()) else {
        return Ok(None);
    };
    let mut command = Command::new("git");
    command.args(["-C", path, "symbolic-ref", "--quiet", "--short", "HEAD"]);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(key);
    }
    command.env("GIT_OPTIONAL_LOCKS", "0");
    // A missing checkout, detached HEAD, unavailable Git, or timed-out Git has no branch.
    match output(&mut command, Duration::from_secs(3)) {
        Ok(result) if result.success => {
            let branch = result.stdout.trim().to_owned();
            Ok((!branch.is_empty()).then_some(branch))
        }
        Ok(_) => Ok(None),
        Err(error) if error.contains("invalid utf-8") => Err(error),
        Err(_) => Ok(None),
    }
}

enum CallError {
    Runtime(String),
    Other(String),
}
impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(e) | Self::Other(e) => f.write_str(e),
        }
    }
}

struct Herdr {
    socket_path: PathBuf,
    binary: OsString,
}

impl Herdr {
    fn new() -> Result<Self, String> {
        let socket_path = PathBuf::from(std::env::var_os("HERDR_SOCKET_PATH").unwrap_or_default());
        if !socket_path.is_absolute() {
            return Err("HERDR_SOCKET_PATH must identify an explicit Herdr session".into());
        }
        Ok(Self {
            socket_path,
            binary: std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into()),
        })
    }

    fn call(&self, args: &[String]) -> Result<Value, CallError> {
        let mut command = Command::new(&self.binary);
        command.args(args).env_remove("HERDR_SESSION");
        let result = output(&mut command, Duration::from_secs(8)).map_err(CallError::Other)?;
        if !result.success {
            let error = result.stderr.trim();
            return Err(CallError::Runtime(if error.is_empty() {
                "Herdr command failed".into()
            } else {
                error.into()
            }));
        }
        if args
            .get(..2)
            .is_some_and(|a| a == ["workspace", "report-metadata"])
            && result.stdout.trim().is_empty()
        {
            return Ok(json!({}));
        }
        let response: Value =
            serde_json::from_str(&result.stdout).map_err(|e| CallError::Other(e.to_string()))?;
        if let Some(error) = response.get("error") {
            return Err(CallError::Runtime(error.to_string()));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| CallError::Other("missing Herdr result".into()))
    }

    fn workspaces(&self) -> Result<Vec<Value>, String> {
        self.call(&["workspace".into(), "list".into()])
            .map_err(|e| e.to_string())?
            .get("workspaces")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| "missing workspace list".into())
    }

    fn publish(&self, workspace: &mut Value, tokens: &Value) -> Result<bool, String> {
        let tokens = tokens.as_object().ok_or("invalid token object")?;
        let old = workspace.get("tokens");
        if tokens
            .iter()
            .all(|(key, value)| old.and_then(|t| t.get(key)).unwrap_or(&Value::Null) == value)
        {
            return Ok(false);
        }
        let workspace_id = text_field(workspace, "workspace_id")?.to_owned();
        let mut args = vec![
            "workspace".into(),
            "report-metadata".into(),
            workspace_id.clone(),
            "--source".into(),
            SOURCE.into(),
        ];
        // Publish the space token before the branch token.
        for key in ["short_space", "short_branch"] {
            if let Some(value) = tokens.get(key) {
                if value.is_null() {
                    args.extend(["--clear-token".into(), key.into()]);
                } else {
                    args.extend([
                        "--token".into(),
                        format!("{key}={}", value.as_str().ok_or("invalid token string")?),
                    ]);
                }
            }
        }
        if let Err(error) = self.call(&args) {
            if let CallError::Runtime(_) = error {
                if !self
                    .workspaces()?
                    .iter()
                    .any(|w| w.get("workspace_id").and_then(Value::as_str) == Some(&workspace_id))
                {
                    return Ok(false);
                }
            }
            return Err(error.to_string());
        }
        let current = workspace
            .as_object_mut()
            .ok_or("invalid workspace")?
            .entry("tokens")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("invalid workspace tokens")?;
        for (key, value) in tokens {
            if value.is_null() {
                current.remove(key);
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
        Ok(true)
    }
}

fn identity_hints(
    herdr: &Herdr,
    workspaces: &mut [Value],
) -> Result<(BTreeMap<String, Value>, u64), String> {
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut initialized = false;
    let mut changed = 0;
    loop {
        let saved = saved_workspaces(&herdr.socket_path);
        let mut hints = BTreeMap::new();
        for workspace in workspaces.iter() {
            let id = text_field(workspace, "workspace_id")?;
            if let Some(hint) = saved.get(id).filter(|h| current_hint(workspace, Some(h))) {
                hints.insert(id.to_owned(), hint.clone());
            }
        }
        if !initialized {
            for workspace in workspaces.iter_mut() {
                if !hints.contains_key(text_field(workspace, "workspace_id")?) {
                    let tokens =
                        json!({"short_space": token_text(Some(text_field(workspace, "label")?))});
                    changed += u64::from(herdr.publish(workspace, &tokens)?);
                }
            }
            initialized = true;
        }
        if hints.len() == workspaces.len() || Instant::now() >= deadline {
            return Ok((hints, changed));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn branches(paths: &[Option<String>]) -> Result<Vec<Option<String>>, String> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..4.min(paths.len()) {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                if index >= paths.len() {
                    break;
                }
                if sender
                    .send((index, branch_at(paths[index].as_deref())))
                    .is_err()
                {
                    break;
                }
            });
        }
        drop(sender);
        let mut results = vec![None; paths.len()];
        for (index, result) in receiver {
            results[index] = Some(result);
        }
        results
            .into_iter()
            .map(|r| r.ok_or_else(|| "Git worker did not return a result".to_owned())?)
            .collect()
    })
}

fn required_env(name: &str) -> Result<OsString, String> {
    std::env::var_os(name).ok_or_else(|| format!("missing {name}"))
}

pub fn refresh() -> Result<u64, String> {
    let herdr = Herdr::new()?;
    let config_dir = PathBuf::from(required_env("HERDR_PLUGIN_CONFIG_DIR")?);
    let config = match std::fs::read_to_string(config_dir.join("config.json")) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.to_string()),
    };
    let formatter = Formatter::new(&config)?;
    let state_dir = PathBuf::from(required_env("HERDR_PLUGIN_STATE_DIR")?);
    std::fs::create_dir_all(&state_dir).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(herdr.socket_path.as_os_str().as_bytes());
    let session_key: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(state_dir.join(format!("{session_key}.lock")))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    let mut workspaces = herdr.workspaces()?;
    let (saved, mut changed) = identity_hints(&herdr, &mut workspaces)?;
    let indented = indented_workspaces(&workspaces)?;
    let renamed_id =
        if std::env::var("HERDR_PLUGIN_EVENT").ok().as_deref() == Some("workspace.renamed") {
            let context: Value = serde_json::from_str(
                &std::env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_else(|_| "{}".into()),
            )
            .map_err(|e| e.to_string())?;
            context
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
    let paths: Vec<_> = workspaces
        .iter()
        .map(|w| {
            w.get("worktree")
                .and_then(|t| t.get("checkout_path"))
                .filter(|v| crate::truthy(v))
                .or_else(|| {
                    w.get("workspace_id")
                        .and_then(Value::as_str)
                        .and_then(|id| saved.get(id))
                        .and_then(|h| h.get("identity_cwd"))
                })
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect();
    let branches = branches(&paths)?;
    for (workspace, branch) in workspaces.iter_mut().zip(branches) {
        let id = text_field(workspace, "workspace_id")?;
        let tokens = labels(
            workspace,
            saved.get(id),
            branch.as_deref(),
            indented.contains(id),
            &formatter,
            renamed_id.as_deref() == Some(id),
        )?;
        changed += u64::from(herdr.publish(workspace, &tokens)?);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::output;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn captures_both_streams_larger_than_pipe_buffers() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            r#"
            index=0
            while [ "$index" -lt 4096 ]; do
                printf '%s\n' 'abcdefghijklmnopqrstuvwxyz0123456789'
                printf '%s\n' 'ABCDEFGHIJKLMNOPQRSTUVWXYZ9876543210' >&2
                index=$((index + 1))
            done
        "#,
        ]);

        let result = output(&mut command, Duration::from_secs(5)).expect("capture both streams");

        assert!(result.success);
        assert_eq!(
            result.stdout,
            "abcdefghijklmnopqrstuvwxyz0123456789\n".repeat(4096)
        );
        assert_eq!(
            result.stderr,
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ9876543210\n".repeat(4096)
        );
    }

    #[test]
    fn times_out_and_reaps_a_running_child() {
        let mut command = Command::new("/bin/sleep");
        command.arg("5");
        let started = Instant::now();

        let error = output(&mut command, Duration::from_millis(100))
            .err()
            .expect("timeout");

        assert!(error.contains("timed out"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout exceeded its deadline"
        );
    }

    struct PipeHolder {
        pid_file: PathBuf,
    }

    impl PipeHolder {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = std::env::temp_dir().join(format!(
                "herdr-branch-labels-pipe-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&directory).expect("create isolated test directory");
            Self {
                pid_file: directory.join("pid"),
            }
        }
    }

    impl Drop for PipeHolder {
        fn drop(&mut self) {
            if let Ok(text) = std::fs::read_to_string(&self.pid_file) {
                if let Ok(pid) = text.trim().parse::<libc::pid_t>() {
                    // This test records only the short-lived descendant it creates.
                    if pid > 0 {
                        unsafe {
                            libc::kill(pid, libc::SIGTERM);
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&self.pid_file);
            let _ = std::fs::remove_dir(self.pid_file.parent().expect("test directory"));
        }
    }

    #[test]
    fn times_out_after_child_exits_with_descendant_holding_pipes() {
        let holder = PipeHolder::new();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            r#"
            sleep 5 &
            printf '%s\n' "$!" > "$1"
            exit 0
        "#,
            "pipe-holder-test",
        ]);
        command.arg(&holder.pid_file);
        let started = Instant::now();

        let error = output(&mut command, Duration::from_millis(100))
            .err()
            .expect("pipe timeout");

        assert!(error.contains("timed out"), "{error}");
        assert!(holder.pid_file.exists(), "descendant was not started");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "open pipes bypassed the deadline"
        );
    }
}

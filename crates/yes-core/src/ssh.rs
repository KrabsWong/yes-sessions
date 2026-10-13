//! App-owned OpenSSH connection and transient native authentication prompts.
use crate::remote::RemoteCodexConfig;
use anyhow::{Context, Result};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::DirBuilderExt,
        net::{UnixListener, UnixStream},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const IPC_ENV: &str = "YES_SESSIONS_ASKPASS_SOCKET";
const MODE_ENV: &str = "YES_SESSIONS_INTERNAL_MODE";
const GUARD_ENV: &str = "YES_SESSIONS_SSH_GUARD";
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);
#[derive(Debug)]
pub enum ConnectionEvent {
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
    Prompt {
        id: u64,
        text: String,
        confirm: bool,
    },
    Failed(String),
    Cancelled,
}
enum Action {
    Answer(u64, Option<String>),
    Retry,
    Stop,
}
pub struct SshConnection {
    path: PathBuf,
    tx: mpsc::Sender<Action>,
    events: Mutex<mpsc::Receiver<ConnectionEvent>>,
    worker: Option<thread::JoinHandle<()>>,
}
impl SshConnection {
    pub fn start(config: RemoteCodexConfig, askpass_executable: PathBuf) -> Result<Self> {
        Self::start_with(config, askpass_executable, master)
    }
    fn start_with(
        config: RemoteCodexConfig,
        askpass_executable: PathBuf,
        make_command: impl FnOnce(&RemoteCodexConfig, &PathBuf, &PathBuf, &PathBuf) -> Command,
    ) -> Result<Self> {
        static NEXT_CONNECTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        config.validate()?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = NEXT_CONNECTION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = PathBuf::from(format!(
            "/tmp/ysc-{}-{nonce:x}-{sequence:x}",
            std::process::id()
        ));
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        let socket = root.join("ask");
        let listener = match UnixListener::bind(&socket).and_then(|listener| {
            listener.set_nonblocking(true)?;
            Ok(listener)
        }) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir_all(&root);
                return Err(error.into());
            }
        };
        let path = root.join("ssh");
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let control = path.clone();
        let command = make_command(&config, &askpass_executable, &socket, &control);
        let worker = thread::spawn(move || {
            manage(
                command,
                askpass_executable,
                control,
                listener,
                rx,
                events_tx,
            );
            let _ = fs::remove_dir_all(root);
        });
        Ok(Self {
            path,
            tx,
            events: Mutex::new(events),
            worker: Some(worker),
        })
    }
    pub fn control_path(&self) -> PathBuf {
        self.path.clone()
    }
    pub fn poll(&self) -> Vec<ConnectionEvent> {
        self.events.lock().unwrap().try_iter().collect()
    }
    pub fn respond(&self, id: u64, answer: Option<String>) {
        let _ = self.tx.send(Action::Answer(id, answer));
    }
    pub fn retry(&self) {
        let _ = self.tx.send(Action::Retry);
    }
    pub fn disconnect(&self) {
        let _ = self.tx.send(Action::Stop);
    }
}
impl Drop for SshConnection {
    fn drop(&mut self) {
        self.disconnect();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn kill_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
fn delay(attempt: u32) -> Duration {
    Duration::from_secs(2u64.saturating_pow(attempt.min(5)).min(30))
}
fn master(
    config: &RemoteCodexConfig,
    exe: &PathBuf,
    socket: &PathBuf,
    control: &PathBuf,
) -> Command {
    let mut c = Command::new("/usr/bin/ssh");
    c.args(["-M", "-N", "-T", "-S"]).arg(control).args([
        "-o",
        "ControlPersist=no",
        "-o",
        "BatchMode=no",
        "-o",
        "ForkAfterAuthentication=no",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "StrictHostKeyChecking=ask",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "RemoteCommand=none",
        "--",
        &config.ssh_alias,
    ]);
    c.env("LC_ALL", "C")
        .env("SSH_ASKPASS", exe)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "yes-sessions")
        .env(IPC_ENV, socket)
        .env(MODE_ENV, "askpass")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    c
}
fn manage(
    mut command: Command,
    exe: PathBuf,
    control: PathBuf,
    listener: UnixListener,
    rx: mpsc::Receiver<Action>,
    events: mpsc::Sender<ConnectionEvent>,
) {
    let mut attempt = 0;
    let mut id = 0;
    'connect: loop {
        let _ = events.send(if attempt == 0 {
            ConnectionEvent::Connecting
        } else {
            ConnectionEvent::Reconnecting { attempt }
        });
        let _ = fs::remove_file(&control);
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = events.send(ConnectionEvent::Failed(e.to_string()));
                break;
            }
        };
        // A separate process observes pipe EOF even if the app crashes or is killed.
        let mut guard = match Command::new(&exe)
            .env(GUARD_ENV, child.id().to_string())
            .env(MODE_ENV, "ssh-guard")
            .env_remove(IPC_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                kill_group(child.id());
                let _ = child.wait();
                let _ = events.send(ConnectionEvent::Failed(e.to_string()));
                break;
            }
        };
        let stderr = child.stderr.take().unwrap();
        let errors = thread::spawn(move || {
            let mut out = Vec::new();
            let mut reader = stderr;
            let mut b = [0; 1024];
            while let Ok(n) = reader.read(&mut b) {
                if n == 0 {
                    break;
                }
                if out.len() < 16384 {
                    out.extend_from_slice(&b[..n.min(16384 - out.len())]);
                }
            }
            String::from_utf8_lossy(&out).trim().to_string()
        });
        let mut pending: Option<(u64, UnixStream, Instant)> = None;
        let mut connected = false;
        let mut prompted = false;
        let mut cancelled = false;
        let mut stopped = false;
        loop {
            while let Ok(action) = rx.try_recv() {
                match action {
                    Action::Stop => stopped = true,
                    Action::Retry => {}
                    Action::Answer(answer_id, answer) => {
                        if pending.as_ref().is_some_and(|p| p.0 == answer_id) {
                            let (_, mut stream, _) = pending.take().unwrap();
                            if let Some(answer) = answer {
                                if answer.len() <= 16384 && !answer.contains(['\n', '\r', '\0']) {
                                    let _ = stream.write_all(&serde_json::to_vec(&answer).unwrap());
                                } else {
                                    cancelled = true;
                                }
                            } else {
                                cancelled = true;
                            }
                        }
                    }
                }
            }
            if pending
                .as_ref()
                .is_some_and(|p| p.2.elapsed() > PROMPT_TIMEOUT)
            {
                cancelled = true;
            }
            if stopped || cancelled {
                break;
            }
            if pending.is_none() {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
                    let mut bytes = Vec::new();
                    if (&mut stream).take(16385).read_to_end(&mut bytes).is_ok()
                        && bytes.len() <= 16384
                    {
                        if let Ok((text, confirm)) =
                            serde_json::from_slice::<(String, bool)>(&bytes)
                        {
                            id += 1;
                            prompted = true;
                            let _ = events.send(ConnectionEvent::Prompt { id, text, confirm });
                            pending = Some((id, stream, Instant::now()));
                        }
                    }
                }
            }
            if !connected && control.exists() {
                connected = true;
                attempt = 0;
                let _ = events.send(ConnectionEvent::Connected);
            }
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                _ => {}
            }
            thread::sleep(Duration::from_millis(50));
        }
        drop(pending);
        kill_group(child.id());
        let _ = child.wait();
        if let Some(mut pipe) = guard.stdin.take() {
            let _ = pipe.write_all(b"done");
        }
        let _ = guard.wait();
        let error = errors.join().unwrap_or_default();
        let _ = fs::remove_file(&control);
        if stopped {
            break;
        }
        if cancelled
            || (!connected
                && (prompted
                    || error.contains("Permission denied")
                    || error.contains("Host key verification failed")
                    || error.contains("REMOTE HOST IDENTIFICATION HAS CHANGED")))
        {
            let _ = events.send(if cancelled {
                ConnectionEvent::Cancelled
            } else {
                ConnectionEvent::Failed(error)
            });
            loop {
                match rx.recv() {
                    Ok(Action::Retry) => {
                        attempt = 0;
                        continue 'connect;
                    }
                    Ok(Action::Stop) | Err(_) => return,
                    _ => {}
                }
            }
        }
        attempt = attempt.saturating_add(1);
        let _ = events.send(ConnectionEvent::Reconnecting { attempt });
        let until = Instant::now() + delay(attempt);
        while Instant::now() < until {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Action::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(Action::Retry) => break,
                _ => {}
            }
        }
    }
}
fn askpass_exchange(mut stream: UnixStream, text: String, confirm: bool) -> Result<String> {
    stream.write_all(&serde_json::to_vec(&(text, confirm))?)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut answer = Vec::new();
    stream.take(65537).read_to_end(&mut answer)?;
    anyhow::ensure!(!answer.is_empty() && answer.len() <= 65536, "Cancelled");
    let answer: String = serde_json::from_slice(&answer)?;
    anyhow::ensure!(
        answer.len() <= 16384 && !answer.contains(['\n', '\r', '\0']),
        "Invalid response"
    );
    Ok(answer)
}
// OpenSSH host-key confirmation uses RP_ECHO and may omit SSH_ASKPASS_PROMPT.
// LC_ALL=C on our SSH process makes these upstream prompt suffixes stable.
fn is_confirmation(text: &str, askpass_prompt: Option<&str>) -> bool {
    askpass_prompt == Some("confirm")
        || text.ends_with("Are you sure you want to continue connecting (yes/no/[fingerprint])? ")
        || text.ends_with("Are you sure you want to continue connecting (yes/no)? ")
        || text == "Please type 'yes', 'no' or the fingerprint: "
        || text == "Please type 'yes' or 'no': "
}
/// Runs before GPUI initialization in helper invocations; answers only travel over IPC/stdout.
pub fn run_askpass() -> Option<i32> {
    let mode = std::env::var(MODE_ENV).ok()?;
    if mode == "ssh-guard" {
        let pid = std::env::var(GUARD_ENV).unwrap_or_default();
        return Some(match pid.parse::<u32>() {
            Ok(pid) => {
                let mut byte = [0];
                if std::io::stdin().read(&mut byte).unwrap_or(0) == 0 {
                    kill_group(pid);
                }
                0
            }
            Err(_) => 1,
        });
    }
    if mode != "askpass" {
        return None;
    }
    let Some(path) = std::env::var_os(IPC_ENV) else {
        return Some(1);
    };
    Some(
        (|| -> Result<()> {
            let stream = UnixStream::connect(path).context("Authentication unavailable")?;
            stream.set_read_timeout(Some(PROMPT_TIMEOUT + Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            let text = std::env::args().nth(1).unwrap_or_default();
            anyhow::ensure!(text.len() < 8192, "Prompt too long");
            let askpass_prompt = std::env::var("SSH_ASKPASS_PROMPT").ok();
            let confirm = is_confirmation(&text, askpass_prompt.as_deref());
            let answer = askpass_exchange(stream, text, confirm)?;
            std::io::stdout().write_all(answer.as_bytes())?;
            std::io::stdout().write_all(b"\n")?;
            Ok(())
        })()
        .map(|_| 0)
        .unwrap_or(1),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(script: &str) -> SshConnection {
        SshConnection::start_with(
            RemoteCodexConfig {
                ssh_alias: "fixture".into(),
                ..Default::default()
            },
            "/usr/bin/true".into(),
            |_, _, _, control| {
                let mut command = Command::new("/bin/sh");
                command
                    .args(["-c", script, "fixture"])
                    .arg(control)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .process_group(0);
                command
            },
        )
        .unwrap()
    }
    fn await_event(
        connection: &SshConnection,
        predicate: impl Fn(&ConnectionEvent) -> bool,
    ) -> ConnectionEvent {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = connection.poll().into_iter().find(|e| predicate(e)) {
                return event;
            }
            assert!(
                Instant::now() < end,
                "Timed out waiting for connection event"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn exited_master_reconnects_and_stop_cleans_socket_directory() {
        let connection = fixture("touch \"$1\"; sleep 0.15");
        let path = connection.control_path();
        await_event(&connection, |e| matches!(e, ConnectionEvent::Connected));
        await_event(&connection, |e| {
            matches!(e, ConnectionEvent::Reconnecting { attempt: 1 })
        });
        await_event(&connection, |e| matches!(e, ConnectionEvent::Connected));
        connection.disconnect();
        drop(connection);
        assert!(!path.parent().unwrap().exists());
    }
    #[test]
    fn cancelled_prompt_stays_stopped_until_explicit_retry() {
        let connection = fixture("sleep 30");
        let mut stream =
            UnixStream::connect(connection.control_path().with_file_name("ask")).unwrap();
        stream.write_all(b"[\"Password:\",false]").unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let event = await_event(&connection, |e| matches!(e, ConnectionEvent::Prompt { .. }));
        let ConnectionEvent::Prompt { id, .. } = event else {
            unreachable!()
        };
        connection.respond(id, None);
        await_event(&connection, |e| matches!(e, ConnectionEvent::Cancelled));
        thread::sleep(Duration::from_millis(2200));
        assert!(
            connection.poll().is_empty(),
            "Cancellation must suppress automatic reconnect"
        );
        connection.retry();
        await_event(&connection, |e| matches!(e, ConnectionEvent::Connecting));
        drop(connection);
    }
    #[test]
    fn drop_terminates_owned_process_group() {
        let connection = fixture("sleep 30 & echo $! > \"$1.pid\"; touch \"$1\"; wait");
        let path = connection.control_path();
        await_event(&connection, |e| matches!(e, ConnectionEvent::Connected));
        let pid = fs::read_to_string(path.with_extension("pid")).unwrap();
        drop(connection);
        let end = Instant::now() + Duration::from_secs(2);
        loop {
            let output = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let status = String::from_utf8_lossy(&output.stdout);
            if status.trim().is_empty() || status.trim().starts_with('Z') {
                break;
            }
            assert!(Instant::now() < end, "Owned child survived disconnect");
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!path.parent().unwrap().exists());
    }
    #[test]
    fn host_key_prompts_use_confirmation_without_askpass_hint() {
        assert!(is_confirmation(
            "The authenticity of host cannot be established.\nED25519 key fingerprint is SHA256:example.\nAre you sure you want to continue connecting (yes/no/[fingerprint])? ",
            None
        ));
        assert!(is_confirmation(
            "Are you sure you want to continue connecting (yes/no)? ",
            None
        ));
        assert!(is_confirmation(
            "Please type 'yes', 'no' or the fingerprint: ",
            None
        ));
        assert!(is_confirmation("Please type 'yes' or 'no': ", None));
        assert!(is_confirmation("Other confirmation", Some("confirm")));
        assert!(!is_confirmation("user@host's password: ", None));
        assert!(!is_confirmation(
            "Enter passphrase for key '/tmp/Are you sure you want to continue connecting': ",
            None
        ));
        assert!(!is_confirmation(
            "Please type your password (yes or no): ",
            None
        ));
    }
    #[test]
    fn authentication_exchange_transfers_prompt_and_empty_password() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || askpass_exchange(client, "Password:".into(), false));
        let mut prompt = Vec::new();
        server.read_to_end(&mut prompt).unwrap();
        assert_eq!(
            serde_json::from_slice::<(String, bool)>(&prompt).unwrap(),
            ("Password:".into(), false)
        );
        server.write_all(b"\"\"").unwrap();
        drop(server);
        assert_eq!(worker.join().unwrap().unwrap(), "");
    }
    #[test]
    fn closed_authentication_request_is_cancellation() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || askpass_exchange(client, "Password:".into(), false));
        server.read_to_end(&mut Vec::new()).unwrap();
        drop(server);
        assert!(worker.join().unwrap().is_err());
    }
    #[test]
    fn invalid_authentication_response_is_rejected() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || askpass_exchange(client, "Password:".into(), false));
        server.read_to_end(&mut Vec::new()).unwrap();
        server.write_all(b"\"bad\\nanswer\"").unwrap();
        drop(server);
        assert!(worker.join().unwrap().is_err());
    }
    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(delay(1).as_secs(), 2);
        assert_eq!(delay(2).as_secs(), 4);
        assert_eq!(delay(100).as_secs(), 30);
    }
    #[test]
    fn master_owns_keepalive_and_never_backgrounds() {
        let c = master(
            &RemoteCodexConfig {
                ssh_alias: "test-host".into(),
                ..Default::default()
            },
            &"/app".into(),
            &"/tmp/ask".into(),
            &"/tmp/control".into(),
        );
        let args: Vec<_> = c
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"ControlPersist=no".into()));
        assert!(args.contains(&"ServerAliveInterval=30".into()));
        assert!(!args.contains(&"-f".into()));
    }
}

//! Read-only Codex sessions over the user's system SSH, with no remote installation.
use crate::{
    AppType, AttachmentSource, Session, SessionDetail, SessionProvider, providers::CodexProvider,
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
const SSH_TIMEOUT: Duration = Duration::from_secs(45);
const LIST_SCRIPT: &str = include_str!("remote/list.sh");
const DETAIL_SCRIPT: &str = include_str!("remote/detail.sh");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteCodexConfig {
    pub ssh_alias: String,
    pub root: String,
}
impl Default for RemoteCodexConfig {
    fn default() -> Self {
        Self {
            ssh_alias: String::new(),
            root: "~/.codex".into(),
        }
    }
}
impl RemoteCodexConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.ssh_alias.is_empty()
                && self.ssh_alias.len() <= 255
                && !self.ssh_alias.starts_with('-')
                && self
                    .ssh_alias
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-@".contains(&c))
                && self.ssh_alias.matches('@').count() <= 1,
            "Enter an SSH config host alias or user@hostname"
        );
        ensure!(
            !self.ssh_alias.starts_with('@') && !self.ssh_alias.ends_with('@'),
            "Invalid SSH destination"
        );
        validate_remote_path(&self.root)
    }
}
fn validate_remote_path(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 4096
            && (value.starts_with('/') || value.starts_with("~/"))
            && !value.chars().any(char::is_control)
            && !Path::new(value)
                .components()
                .any(|c| c == Component::ParentDir),
        "Remote Codex root must be absolute or start with ~/ and cannot contain .. or control characters"
    );
    Ok(())
}
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn private_dir(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let meta = fs::symlink_metadata(path)?;
    let home = dirs::home_dir().context("Home directory unavailable")?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == fs::metadata(home)?.uid(),
        "Unsafe SSH cache directory"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn control_path(destination: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Home directory unavailable")?;
    // Leave room for OpenSSH's temporary suffix (a dot and 16 random characters)
    // within macOS's 104-byte sockaddr_un.sun_path, including the terminating NUL.
    // Keep full hashes in a private, owner-checked directory independent of home length.
    let root = PathBuf::from(format!(
        "/tmp/ys-{:x}",
        md5::compute(home.as_os_str().as_encoded_bytes())
    ));
    private_dir(&root)?;
    Ok(root.join(format!("{:x}", md5::compute(destination))))
}

#[derive(Debug)]
struct Cache {
    root: PathBuf,
    remote_root: PathBuf,
    files: Vec<RemoteFile>,
    sessions: Vec<Session>,
    loaded: bool,
}
impl Cache {
    fn new() -> Result<Self> {
        static NEXT_CACHE: AtomicU64 = AtomicU64::new(0);
        let parent = control_path("cache")?.parent().unwrap().to_path_buf();
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = NEXT_CACHE.fetch_add(1, Ordering::Relaxed);
        let root = parent.join(format!("data-{}-{suffix}-{sequence}", std::process::id()));
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        fs::create_dir(root.join("sessions"))?;
        let root = root.canonicalize()?;
        Ok(Self {
            root,
            remote_root: PathBuf::new(),
            files: Vec::new(),
            sessions: Vec::new(),
            loaded: false,
        })
    }
}
impl Drop for Cache {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[derive(Debug)]
struct RemoteFile {
    relative: PathBuf,
    id: Option<String>,
    modified: i64,
}
#[derive(Debug, Clone)]
pub struct RemoteCodexProvider {
    config: RemoteCodexConfig,
    connection: PathBuf,
    cache: Arc<Mutex<Cache>>,
    cancellation: Arc<AtomicU64>,
}
impl RemoteCodexProvider {
    pub fn new(config: RemoteCodexConfig) -> Result<Self> {
        let path = control_path(&config.ssh_alias)?;
        Self::with_connection(config, path)
    }
    pub fn with_connection(config: RemoteCodexConfig, connection: PathBuf) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            connection,
            cache: Arc::new(Mutex::new(Cache::new()?)),
            cancellation: Arc::new(AtomicU64::new(0)),
        })
    }
    pub fn cancel(&self) {
        self.cancellation.fetch_add(1, Ordering::Relaxed);
    }
    fn request(&self, script: &str, paths: &[PathBuf], generation: u64) -> Result<Vec<u8>> {
        let mut remote_command = format!(
            "/bin/bash -c {} yes-sessions {}",
            shell_quote(script),
            shell_quote(&self.config.root)
        );
        for path in paths {
            let text = path
                .to_str()
                .context("Remote session filename is not UTF-8")?;
            let quoted = shell_quote(text);
            ensure!(
                remote_command.len() + quoted.len() < 128 * 1024,
                "Too many remote session segments"
            );
            remote_command.push(' ');
            remote_command.push_str(&quoted);
        }
        let socket = &self.connection;
        let mut command = Command::new("/usr/bin/ssh");
        command
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "ProxyCommand=/usr/bin/false",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=10",
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
                "-o",
                "ServerAliveInterval=10",
                "-o",
                "ServerAliveCountMax=2",
                "-o",
                "ControlMaster=no",
                "-S",
            ])
            .arg(socket)
            .args(["--", &self.config.ssh_alias, &remote_command]);
        run_bounded(command, SSH_TIMEOUT, &self.cancellation, generation)
    }
    fn load_list(&self, cache: &mut Cache, generation: u64) -> Result<()> {
        let output = self.request(LIST_SCRIPT, &[], generation)?;
        let snapshot = parse_snapshot(&output, false)?;
        fs::remove_dir_all(cache.root.join("sessions"))?;
        fs::create_dir(cache.root.join("sessions"))?;
        fs::write(cache.root.join("session_index.jsonl"), snapshot.index)?;
        let mut files = Vec::new();
        for entry in snapshot.files {
            let path = cache.root.join(&entry.relative);
            fs::create_dir_all(path.parent().unwrap())?;
            let id = entry
                .prefix
                .split(|byte| *byte == b'\n')
                .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
                .find(|value| value.get("type").and_then(|v| v.as_str()) == Some("session_meta"))
                .and_then(|value| {
                    value
                        .pointer("/payload/id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                });
            let mut content = entry.prefix;
            // Preserve a complete final line for summary timestamps, while dropping partial
            // prefix/tail records. This mirror is exclusively a summary, never detail data.
            if entry.size > content.len() as u64 {
                if let Some(end) = content.iter().rposition(|byte| *byte == b'\n') {
                    content.truncate(end + 1);
                } else {
                    content.clear();
                }
                if let Some(start) = entry.tail.iter().position(|byte| *byte == b'\n') {
                    content.extend_from_slice(&entry.tail[start + 1..]);
                }
            }
            fs::write(path, content)?;
            files.push(RemoteFile {
                relative: entry.relative,
                id,
                modified: entry.modified,
            });
        }
        let mut sessions = CodexProvider::for_remote(cache.root.clone()).sessions()?;
        for session in &mut sessions {
            remap_session(session, &cache.root, &snapshot.root, &files)?;
        }
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        cache.remote_root = snapshot.root;
        cache.files = files;
        cache.sessions = sessions;
        cache.loaded = true;
        Ok(())
    }
    fn detail(&self, id: &str, with_usage: bool) -> Result<Option<SessionDetail>> {
        ensure!(
            !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
            "Invalid session ID"
        );
        let generation = self.cancellation.load(Ordering::Relaxed);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Remote cache lock failed"))?;
        if !cache.loaded {
            self.load_list(&mut cache, generation)?;
        }
        let mut ids = HashSet::from([id.to_owned()]);
        if with_usage {
            loop {
                let before = ids.len();
                for session in &cache.sessions {
                    if session
                        .parent_session_id
                        .as_ref()
                        .is_some_and(|parent| ids.contains(parent))
                    {
                        ids.insert(session.id.clone());
                    }
                }
                if before == ids.len() {
                    break;
                }
            }
        }
        let mut downloaded = HashSet::new();
        loop {
            let paths = cache
                .files
                .iter()
                .filter(|file| {
                    file.id.as_ref().is_some_and(|id| ids.contains(id))
                        || ids.iter().any(|id| {
                            file.relative
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.ends_with(&format!("-{id}.jsonl")))
                        })
                })
                .filter(|file| !downloaded.contains(&file.relative))
                .map(|file| file.relative.clone())
                .collect::<Vec<_>>();
            if paths.is_empty() {
                break;
            }
            let output = self.request(DETAIL_SCRIPT, &paths, generation)?;
            let snapshot = parse_snapshot(&output, true)?;
            ensure!(
                snapshot.root == cache.remote_root,
                "Remote root changed; refresh the source"
            );
            ensure!(
                snapshot.files.len() == paths.len(),
                "Incomplete remote detail response"
            );
            for file in snapshot.files {
                ensure!(
                    paths.contains(&file.relative),
                    "Unexpected remote session path"
                );
                fs::write(cache.root.join(&file.relative), file.prefix)?;
                downloaded.insert(file.relative);
            }
            if !with_usage {
                break;
            }
            let provider = CodexProvider::for_remote(cache.root.clone());
            let known = ids.iter().cloned().collect::<Vec<_>>();
            for id in known {
                if let Some(detail) = provider.session_detail(&id)? {
                    ids.extend(
                        detail
                            .messages
                            .into_iter()
                            .filter_map(|message| message.sub_agent_session_id),
                    );
                }
            }
            ensure!(ids.len() <= 2000, "Too many remote child sessions");
        }
        if downloaded.is_empty() {
            return Ok(None);
        }
        // All required segments are now local, so subtree accounting does not incur SSH per message.
        let provider = CodexProvider::for_remote(cache.root.clone());
        let mut detail = if with_usage {
            provider.session_detail_with_usage(id)?
        } else {
            provider.session_detail(id)?
        };
        if let Some(detail) = &mut detail {
            remap_session(
                &mut detail.session,
                &cache.root,
                &cache.remote_root,
                &cache.files,
            )?;
            sanitize_detail(detail);
        }
        Ok(detail)
    }
}
impl SessionProvider for RemoteCodexProvider {
    fn app_type(&self) -> AppType {
        AppType::Codex
    }
    fn is_available(&self) -> bool {
        true
    }
    fn sessions(&self) -> Result<Vec<Session>> {
        let generation = self.cancellation.load(Ordering::Relaxed);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Remote cache lock failed"))?;
        let mut next = Cache::new()?;
        self.load_list(&mut next, generation)?;
        *cache = next;
        Ok(cache.sessions.clone())
    }
    fn session_detail(&self, id: &str) -> Result<Option<SessionDetail>> {
        self.detail(id, false)
    }
    fn session_detail_with_usage(&self, id: &str) -> Result<Option<SessionDetail>> {
        self.detail(id, true)
    }
    fn session_detail_with_usage_from_sessions(
        &self,
        id: &str,
        _sessions: &[Session],
    ) -> Result<Option<SessionDetail>> {
        self.detail(id, true)
    }
}
fn remap_session(
    session: &mut Session,
    local_root: &Path,
    remote_root: &Path,
    files: &[RemoteFile],
) -> Result<()> {
    let relative = session.file_path.strip_prefix(local_root)?;
    if let Some(file) = files.iter().find(|file| file.relative == relative) {
        session.updated_at = file.modified.saturating_mul(1000);
    }
    session.file_path = remote_root.join(relative);
    Ok(())
}
fn sanitize_detail(detail: &mut SessionDetail) {
    for message in &mut detail.messages {
        message
            .attachments
            .retain(|attachment| matches!(attachment.source, AttachmentSource::DataUrl(_)));
    }
}

struct WireFile {
    relative: PathBuf,
    modified: i64,
    size: u64,
    prefix: Vec<u8>,
    tail: Vec<u8>,
}
struct Snapshot {
    root: PathBuf,
    index: Vec<u8>,
    files: Vec<WireFile>,
}
fn decode(value: &str) -> Result<Vec<u8>> {
    STANDARD
        .decode(value)
        .context("Invalid SSH response encoding")
}
fn parse_snapshot(bytes: &[u8], detail: bool) -> Result<Snapshot> {
    let text = std::str::from_utf8(bytes)
        .context("Invalid SSH response; check remote shell startup output")?;
    let mut ended = false;
    let mut root = None;
    let mut index = Vec::new();
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        ensure!(!ended, "Unexpected data after SSH snapshot");
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["END"] => ended = true,
            ["ROOT", encoded] if root.is_none() => {
                let path = PathBuf::from(String::from_utf8(decode(encoded)?)?);
                ensure!(path.is_absolute(), "Invalid remote root response");
                root = Some(path);
            }
            ["INDEX", encoded] if !detail => {
                index = decode(encoded)?;
                ensure!(
                    index.len() <= 4 * 1024 * 1024,
                    "Remote session index is too large"
                );
            }
            ["FILE", encoded, modified, size, prefix, rest @ ..] => {
                ensure!(
                    root.is_some() && rest.len() == usize::from(!detail),
                    "Invalid SSH file response"
                );
                let relative = PathBuf::from(String::from_utf8(decode(encoded)?)?);
                ensure!(
                    relative.starts_with("sessions")
                        && relative
                            .components()
                            .all(|c| matches!(c, Component::Normal(_)))
                        && relative.extension().is_some_and(|ext| ext == "jsonl")
                        && seen.insert(relative.clone()),
                    "Invalid or duplicate remote session path"
                );
                let prefix = decode(prefix)?;
                let tail = if detail { Vec::new() } else { decode(rest[0])? };
                ensure!(
                    prefix.len() <= if detail { 32 * 1024 * 1024 } else { 256 * 1024 }
                        && tail.len() <= 64 * 1024,
                    "Remote session exceeded transfer limit"
                );
                let size = size.parse()?;
                ensure!(
                    !detail || prefix.len() as u64 == size,
                    "Remote file changed while being read; refresh and retry"
                );
                files.push(WireFile {
                    relative,
                    modified: modified.parse()?,
                    size,
                    prefix,
                    tail,
                });
                ensure!(files.len() <= 2000, "Too many remote session files");
            }
            _ => bail!("Invalid SSH response; check remote shell startup output"),
        }
    }
    ensure!(ended, "Incomplete SSH snapshot");
    Ok(Snapshot {
        root: root.context("Empty SSH response")?,
        index,
        files,
    })
}
fn bounded_read(mut reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "SSH output exceeded the {limit}-byte limit"
    );
    Ok(bytes)
}
fn run_bounded(
    mut command: Command,
    timeout: Duration,
    cancellation: &AtomicU64,
    generation: u64,
) -> Result<Vec<u8>> {
    ensure!(
        cancellation.load(Ordering::Relaxed) == generation,
        "SSH request cancelled"
    );
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not launch SSH")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let output_tx = tx.clone();
    thread::spawn(move || {
        let _ = output_tx.send((true, bounded_read(stdout, RESPONSE_LIMIT)));
    });
    thread::spawn(move || {
        let _ = tx.send((false, bounded_read(stderr, STDERR_LIMIT)));
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let mut output = None;
        let mut error_output = None;
        while output.is_none() || error_output.is_none() {
            ensure!(
                cancellation.load(Ordering::Relaxed) == generation,
                "SSH request cancelled"
            );
            ensure!(
                Instant::now() < deadline,
                "SSH request timed out; check connectivity and remote session size"
            );
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok((kind, result)) => {
                    let bytes = result?;
                    if kind {
                        output = Some(bytes);
                    } else {
                        error_output = Some(bytes);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            ensure!(
                Instant::now() < deadline && cancellation.load(Ordering::Relaxed) == generation,
                "SSH request timed out or was cancelled"
            );
            thread::sleep(Duration::from_millis(10));
        };
        ensure!(
            status.success(),
            "SSH session read failed: {}",
            String::from_utf8_lossy(&error_output.unwrap_or_default()).trim()
        );
        Ok(output.unwrap_or_default())
    })();
    if result.is_err() {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
    let _ = child.wait();
    result
}

/// Refuse symlinks at every component below a selected local mirror root.
pub(crate) fn checked_path(root: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(root)
        .context("Session path is outside its root")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        ensure!(
            matches!(component, Component::Normal(_)),
            "Invalid session path component"
        );
        current.push(component);
        ensure!(
            !fs::symlink_metadata(&current)?.file_type().is_symlink(),
            "Session paths cannot contain symlinks"
        );
    }
    let canonical = path.canonicalize()?;
    ensure!(
        canonical.starts_with(root),
        "Session path is outside its root"
    );
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_socket_fits_with_openssh_temporary_suffix() {
        use std::os::unix::net::UnixListener;

        let destination = format!("socket-regression-{}@example.com", std::process::id());
        let socket = control_path(&destination).unwrap();
        assert_eq!(socket, control_path(&destination).unwrap());
        assert_ne!(socket, control_path("another-host").unwrap());
        let temporary = PathBuf::from(format!("{}.0123456789abcdef", socket.display()));
        assert!(temporary.as_os_str().as_encoded_bytes().len() < 104);
        let listener = UnixListener::bind(&temporary).unwrap();
        drop(listener);
        fs::remove_file(temporary).unwrap();
    }

    fn shell_snapshot(script: &str, root: &Path, paths: &[&str]) -> Result<Vec<u8>> {
        let script = if cfg!(target_os = "macos") {
            script
                .replace("stat -c \"%Y %s\" --", "stat -f \"%m %z\"")
                .replace("stat -c %s --", "stat -f %z")
                .replace("stat -c %Y --", "stat -f %m")
        } else {
            script.to_owned()
        };
        let mut command = Command::new("/bin/bash");
        command
            .args(["-c", &script, "yes-test"])
            .arg(root)
            .args(paths);
        run_bounded(command, Duration::from_secs(10), &AtomicU64::new(0), 0)
    }

    #[test]
    fn shell_list_is_bounded_and_detail_is_complete_with_quoted_paths() {
        let cache = Cache::new().unwrap();
        let root = cache.root.join("codex ' 中文 $(echo unsafe)");
        let directory = root.join("sessions/2026/10");
        fs::create_dir_all(&directory).unwrap();
        let relative = "sessions/2026/10/rollout-quoted ' 中文.jsonl";
        let meta = serde_json::json!({"type":"session_meta", "payload":{"id":"test-thread", "timestamp":"2026-10-10T01:00:00Z", "cwd":"/remote/project"}});
        let user = serde_json::json!({"type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text", "text":"Hello remote"}]}});
        let filler = serde_json::json!({"type":"event_msg", "payload":{"type":"agent_message", "message":"x".repeat(400_000)}});
        let source = format!("{meta}\n{user}\n{filler}\n");
        fs::write(root.join(relative), &source).unwrap();
        let listing = shell_snapshot(LIST_SCRIPT, &root, &[]).unwrap();
        let snapshot = parse_snapshot(&listing, false).unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[0].relative, PathBuf::from(relative));
        assert_eq!(snapshot.files[0].prefix.len(), 256 * 1024);
        assert_eq!(snapshot.files[0].tail.len(), 64 * 1024);
        let detail = shell_snapshot(DETAIL_SCRIPT, &root, &[relative]).unwrap();
        let snapshot = parse_snapshot(&detail, true).unwrap();
        assert_eq!(snapshot.files[0].prefix, source.as_bytes());
        assert!(parse_snapshot(&detail[..detail.len() - 4], true).is_err());
        // A private mirror must work even when /tmp is a symlink on macOS.
        fs::write(
            cache.root.join("sessions/rollout-test-thread.jsonl"),
            source,
        )
        .unwrap();
        let provider = CodexProvider::for_remote(cache.root.clone());
        assert_eq!(provider.sessions().unwrap().len(), 1);
        assert!(provider.session_detail("test-thread").unwrap().is_some());
    }

    #[test]
    fn shell_rejects_symlinks_and_oversized_details() {
        use std::os::unix::fs::symlink;
        let cache = Cache::new().unwrap();
        let outside = cache.root.join("outside.jsonl");
        fs::write(&outside, "secret").unwrap();
        symlink(&outside, cache.root.join("sessions/link.jsonl")).unwrap();
        assert!(shell_snapshot(DETAIL_SCRIPT, &cache.root, &["sessions/link.jsonl"]).is_err());
        let listing = shell_snapshot(LIST_SCRIPT, &cache.root, &[]).unwrap();
        assert!(parse_snapshot(&listing, false).unwrap().files.is_empty());
        symlink(&cache.root, cache.root.join("sessions/linked-dir")).unwrap();
        assert!(
            shell_snapshot(
                DETAIL_SCRIPT,
                &cache.root,
                &["sessions/linked-dir/outside.jsonl"]
            )
            .is_err()
        );
        let large = fs::File::create(cache.root.join("sessions/large.jsonl")).unwrap();
        large.set_len(33 * 1024 * 1024).unwrap();
        assert!(shell_snapshot(DETAIL_SCRIPT, &cache.root, &["sessions/large.jsonl"]).is_err());
    }

    #[test]
    fn rejects_destination_and_path_injection() {
        let config = RemoteCodexConfig {
            ssh_alias: "ubuntu@example.com".into(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
        for alias in ["-oProxyCommand=evil", "host;id", "host\nnext", "*", "x@@y"] {
            let mut value = config.clone();
            value.ssh_alias = alias.into();
            assert!(value.validate().is_err());
        }
        for root in ["relative", "/tmp/../etc", "~/x\ny"] {
            let mut value = config.clone();
            value.root = root.into();
            assert!(value.validate().is_err());
        }
        assert_eq!(shell_quote("x' $(touch x)"), "'x'\\'' $(touch x)'");
    }
    #[test]
    fn rejects_untrusted_wire_paths_and_limits() {
        let header = format!("ROOT\t{}\n", STANDARD.encode("/home/test/.codex"));
        for path in [
            "/etc/passwd.jsonl",
            "sessions/../secret.jsonl",
            "other/file.jsonl",
        ] {
            let wire = format!("{header}FILE\t{}\t123\t0\t\t\n", STANDARD.encode(path));
            assert!(parse_snapshot(wire.as_bytes(), false).is_err());
        }
        let wire = format!(
            "{header}FILE\t{}\t123\t3\t{}\t\n",
            STANDARD.encode("sessions/a.jsonl"),
            STANDARD.encode("abc")
        );
        let wire = format!("{wire}END\n");
        let snapshot = parse_snapshot(wire.as_bytes(), false).unwrap();
        assert_eq!(snapshot.files[0].prefix, b"abc");
        assert!(bounded_read(&b"12345"[..], 4).is_err());
    }
    #[test]
    fn transport_timeout_and_cancel_reap_child() {
        let mut command = Command::new("/bin/sleep");
        command.arg("30");
        let start = Instant::now();
        assert!(run_bounded(command, Duration::from_millis(100), &AtomicU64::new(0), 0).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            run_bounded(
                Command::new("/bin/echo"),
                Duration::from_secs(1),
                &AtomicU64::new(1),
                0
            )
            .is_err()
        );
    }
}

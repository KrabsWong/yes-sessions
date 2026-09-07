//! Read-only, bounded previews rooted at a session's working directory.
use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{Read, Write},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_GIT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RESULTS: usize = 500;
const MAX_TEXT_LINES: usize = 100_000;
const MAX_LINE_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeScope {
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub ignored: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Tiff,
    Ico,
    Svg,
}

fn image_format(path: &Path, bytes: &[u8]) -> Option<PreviewImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(PreviewImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(PreviewImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(PreviewImageFormat::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(PreviewImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(PreviewImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(PreviewImageFormat::Tiff)
    } else if bytes.starts_with(b"\0\0\x01\0") {
        Some(PreviewImageFormat::Ico)
    } else {
        match path
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str()
        {
            "png" => Some(PreviewImageFormat::Png),
            "jpg" | "jpeg" => Some(PreviewImageFormat::Jpeg),
            "gif" => Some(PreviewImageFormat::Gif),
            "webp" => Some(PreviewImageFormat::Webp),
            "bmp" => Some(PreviewImageFormat::Bmp),
            "tif" | "tiff" => Some(PreviewImageFormat::Tiff),
            "ico" => Some(PreviewImageFormat::Ico),
            "svg" => Some(PreviewImageFormat::Svg),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum FileContent {
    Text(String),
    Image {
        format: PreviewImageFormat,
        bytes: Vec<u8>,
    },
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub scope: ChangeScope,
    pub status: String,
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
}

fn valid_relative(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        bail!("Path must stay inside the working directory")
    }
    Ok(())
}

fn resolve(root: &Path, relative: &Path) -> Result<PathBuf> {
    valid_relative(relative)?;
    let root = root
        .canonicalize()
        .context("Working directory is unavailable")?;
    let path = root
        .join(relative)
        .canonicalize()
        .context("File is unavailable")?;
    if !path.starts_with(root) {
        bail!("Path points outside the working directory")
    }
    Ok(path)
}

pub fn entries(root: &Path, relative: &Path) -> Result<Vec<Entry>> {
    let directory = resolve(root, relative)?;
    let mut result = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = relative.join(entry.file_name());
        let Ok(resolved) = resolve(root, &path) else {
            continue;
        };
        let metadata = fs::metadata(resolved)?;
        if metadata.is_dir() || metadata.is_file() {
            result.push(Entry {
                path,
                is_dir: metadata.is_dir(),
                ignored: false,
            });
        }
        if result.len() >= 10_000 {
            break;
        }
    }
    result.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    mark_ignored(root, &mut result);
    Ok(result)
}

pub fn search(root: &Path, query: &str) -> Result<Vec<Entry>> {
    let canonical = root.canonicalize()?;
    let query = query.to_lowercase();
    let mut result = Vec::new();
    let walker = walkdir::WalkDir::new(&canonical)
        .follow_links(false)
        .max_depth(32)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walker.take(100_000) {
        let Ok(entry) = entry else {
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().strip_prefix(&canonical)?.to_path_buf();
        if path.to_string_lossy().to_lowercase().contains(&query) {
            result.push(Entry {
                path,
                is_dir: false,
                ignored: false,
            });
            if result.len() == MAX_RESULTS {
                break;
            }
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    mark_ignored(root, &mut result);
    Ok(result)
}

// Batch only the loaded entries. Git handles nested rules, negation, and tracked files.
fn mark_ignored(root: &Path, entries: &mut [Entry]) {
    if entries.is_empty() {
        return;
    }
    let Ok(mut child) = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "check-ignore",
            "--stdin",
            "-z",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let mut input = Vec::new();
    for entry in entries.iter() {
        input.extend_from_slice(entry.path.as_os_str().as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take().unwrap();
    // Drain stdout while feeding stdin so large directories cannot fill both pipes.
    let output = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let mut output = Vec::new();
        let read = child
            .stdout
            .take()
            .unwrap()
            .take(MAX_GIT_BYTES + 1)
            .read_to_end(&mut output);
        if read.is_err() || output.len() as u64 > MAX_GIT_BYTES {
            let _ = child.kill();
        }
        let status = child.wait();
        let wrote = matches!(writer.join(), Ok(Ok(())));
        (wrote
            && read.is_ok()
            && output.len() as u64 <= MAX_GIT_BYTES
            && status.is_ok_and(|status| status.success()))
        .then_some(output)
    });
    if let Some(output) = output {
        let ignored: HashSet<_> = output
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| PathBuf::from(OsString::from_vec(path.to_vec())))
            .collect();
        for entry in entries {
            entry.ignored = ignored.contains(&entry.path);
        }
    }
}

pub fn read_file(root: &Path, relative: &Path) -> Result<FileContent> {
    let path = resolve(root, relative)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() {
        return Ok(FileContent::Unsupported);
    }
    let file = fs::File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Ok(FileContent::Unsupported);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(FileContent::Unsupported);
    }
    if let Some(format) = image_format(relative, &bytes) {
        return Ok(FileContent::Image { format, bytes });
    }
    let extension = relative
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if bytes
        .iter()
        .any(|byte| *byte < 0x20 && !matches!(*byte, b'\t' | b'\n' | b'\r'))
        || bytes.starts_with(b"%PDF-")
        || matches!(
            extension.as_str(),
            "pdf"
                | "zip"
                | "gz"
                | "tar"
                | "7z"
                | "rar"
                | "bz2"
                | "xz"
                | "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "mp3"
                | "mp4"
                | "mov"
                | "wav"
                | "heic"
                | "heif"
                | "avif"
        )
    {
        return Ok(FileContent::Unsupported);
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) if supported_text(&text) => FileContent::Text(text),
        Ok(_) => FileContent::Unsupported,
        Err(_) => FileContent::Unsupported,
    })
}

fn supported_text(text: &str) -> bool {
    text.lines()
        .take(MAX_TEXT_LINES + 1)
        .enumerate()
        .all(|(index, line)| index < MAX_TEXT_LINES && line.len() <= MAX_LINE_BYTES)
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "color.ui=false",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn git(root: &Path, args: &[OsString]) -> Result<Vec<u8>> {
    let mut child = git_command(root).args(args).spawn()?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(MAX_GIT_BYTES + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_GIT_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        bail!("Git preview exceeds the size limit or could not be read")
    }
    if !child.wait()?.success() {
        bail!("Git preview is unavailable for this directory")
    }
    Ok(bytes)
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

struct Record {
    change: Change,
    old_path: Option<PathBuf>,
}

fn status(root: &Path) -> Result<Vec<u8>> {
    git(
        root,
        &args(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ]),
    )
}

fn records(root: &Path) -> Result<Vec<Record>> {
    records_from_status(root, &status(root)?)
}

fn records_from_status(root: &Path, output: &[u8]) -> Result<Vec<Record>> {
    let root = root.canonicalize()?;
    let top = git(&root, &args(&["rev-parse", "--show-toplevel"]))?;
    let top = top.strip_suffix(b"\n").unwrap_or(&top);
    let top = PathBuf::from(OsString::from_vec(top.to_vec())).canonicalize()?;
    let prefix = root.strip_prefix(&top)?;
    let mut fields = output.split(|byte| *byte == 0);
    let mut result = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let path = PathBuf::from(OsString::from_vec(field[3..].to_vec()));
        let old = if matches!(field[0], b'R' | b'C') || matches!(field[1], b'R' | b'C') {
            fields
                .next()
                .map(|old| PathBuf::from(OsString::from_vec(old.to_vec())))
        } else {
            None
        };
        let Ok(path) = path.strip_prefix(prefix) else {
            continue;
        };
        valid_relative(path)?;
        let old_path = old.and_then(|p| p.strip_prefix(prefix).ok().map(Path::to_path_buf));
        for (code, scope) in [
            (field[0], ChangeScope::Staged),
            (field[1], ChangeScope::Unstaged),
        ] {
            if code == b' ' || code == b'?' {
                continue;
            }
            result.push(Record {
                change: Change {
                    path: path.to_path_buf(),
                    scope,
                    status: char::from(code).to_string(),
                    additions: None,
                    deletions: None,
                },
                old_path: if matches!(code, b'R' | b'C') {
                    old_path.clone()
                } else {
                    None
                },
            });
        }
        if &field[..2] == b"??" {
            result.push(Record {
                change: Change {
                    path: path.to_path_buf(),
                    scope: ChangeScope::Untracked,
                    status: "?".into(),
                    additions: None,
                    deletions: None,
                },
                old_path: None,
            });
        }
    }
    Ok(result)
}

fn numstat(
    root: &Path,
    scope: ChangeScope,
) -> Result<std::collections::HashMap<PathBuf, (Option<usize>, Option<usize>)>> {
    let mut command = args(&[
        "diff",
        "--numstat",
        "-z",
        "--relative",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
    ]);
    if scope == ChangeScope::Staged {
        command.push("--cached".into());
    }
    command.extend(args(&["--", "."]));
    let output = git(root, &command)?;
    let mut fields = output.split(|byte| *byte == 0);
    let mut result = std::collections::HashMap::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let mut parts = field.splitn(3, |byte| *byte == b'\t');
        let additions = parts
            .next()
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse().ok());
        let deletions = parts
            .next()
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse().ok());
        let Some(mut path) = parts.next() else {
            continue;
        };
        if path.is_empty() {
            // Renames have separate NUL-delimited source and destination paths.
            let _ = fields.next();
            let Some(destination) = fields.next() else {
                continue;
            };
            path = destination;
        }
        result.insert(
            PathBuf::from(OsString::from_vec(path.to_vec())),
            (additions, deletions),
        );
    }
    Ok(result)
}

pub fn changes(root: &Path) -> Result<Vec<Change>> {
    let mut changes: Vec<_> = records(root)?
        .into_iter()
        .map(|record| record.change)
        .collect();
    for scope in [ChangeScope::Staged, ChangeScope::Unstaged] {
        if !changes.iter().any(|change| change.scope == scope) {
            continue;
        }
        let counts = numstat(root, scope)?;
        for change in changes.iter_mut().filter(|change| change.scope == scope) {
            if let Some((additions, deletions)) = counts.get(&change.path) {
                change.additions = *additions;
                change.deletions = *deletions;
            }
        }
    }
    Ok(changes)
}

// Probe magic bytes before copying a Git blob into memory. Text previews do not
// need either complete image version; a single cat-file replaces size + show.
fn revision_image(root: &Path, spec: OsString, path: &Path) -> Result<Option<FileContent>> {
    let mut child = git_command(root)
        .args(["cat-file".into(), "blob".into(), spec])
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let mut bytes = Vec::new();
    if let Err(error) = stdout.by_ref().take(12).read_to_end(&mut bytes) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let Some(format) = image_format(path, &bytes) else {
        // An empty stdout may be a missing revision (e.g. an added/deleted side).
        if bytes.is_empty() {
            return Ok(child.wait()?.success().then_some(FileContent::Unsupported));
        }
        let _ = child.kill();
        let _ = child.wait();
        return Ok(Some(FileContent::Unsupported));
    };
    let result = stdout
        .take(MAX_FILE_BYTES + 1 - bytes.len() as u64)
        .read_to_end(&mut bytes);
    if result.is_err() || bytes.len() as u64 > MAX_FILE_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        result?;
        return Ok(Some(FileContent::Unsupported));
    }
    if !child.wait()?.success() {
        return Ok(None);
    }
    Ok(Some(FileContent::Image { format, bytes }))
}

fn worktree_image(root: &Path, relative: &Path) -> Result<Option<FileContent>> {
    if !root.join(relative).try_exists()? {
        return Ok(None);
    }
    let path = resolve(root, relative)?;
    if !fs::metadata(&path)?.is_file() {
        // Gitlinks are directories in the worktree; let the ordinary Git diff
        // show their commit change without opening a directory or special file.
        return Ok(Some(FileContent::Unsupported));
    }
    let mut prefix = Vec::new();
    fs::File::open(path)?.take(12).read_to_end(&mut prefix)?;
    if image_format(relative, &prefix).is_none() {
        return Ok(Some(FileContent::Unsupported));
    }
    Ok(Some(read_file(root, relative)?))
}

/// Read the two image versions for the selected Git scope. Missing sides represent
/// additions/deletions; oversized or non-image sides remain unsupported.
pub fn image_diff(
    root: &Path,
    relative: &Path,
    scope: ChangeScope,
) -> Result<Option<(Option<FileContent>, Option<FileContent>)>> {
    let root = root.canonicalize()?;
    valid_relative(relative)?;
    let old_path = if scope == ChangeScope::Untracked {
        relative.to_path_buf()
    } else {
        records(&root)?
            .into_iter()
            .find(|r| r.change.path == relative && r.change.scope == scope)
            .and_then(|r| r.old_path)
            .unwrap_or_else(|| relative.to_path_buf())
    };
    let prefix = if scope == ChangeScope::Untracked {
        Vec::new()
    } else {
        git(&root, &args(&["rev-parse", "--show-prefix"]))?
    };
    let prefix = prefix.strip_suffix(b"\n").unwrap_or(&prefix);
    let read_revision = |revision: &str, path: &Path| -> Result<Option<FileContent>> {
        let mut spec = revision.as_bytes().to_vec();
        spec.push(b':');
        spec.extend_from_slice(prefix);
        spec.extend_from_slice(path.as_os_str().as_bytes());
        revision_image(&root, OsString::from_vec(spec), path)
    };
    let before = match scope {
        ChangeScope::Staged => read_revision("HEAD", &old_path)?,
        ChangeScope::Unstaged => read_revision("", &old_path)?,
        ChangeScope::Untracked => None,
    };
    let after = if scope == ChangeScope::Staged {
        read_revision("", relative)?
    } else {
        worktree_image(&root, relative)?
    };
    if image_format(relative, &[]).is_none()
        && image_format(&old_path, &[]).is_none()
        && !matches!(before, Some(FileContent::Image { .. }))
        && !matches!(after, Some(FileContent::Image { .. }))
    {
        return Ok(None);
    }
    Ok(Some((before, after)))
}

/// A cheap change signal: status/index plus changed-file metadata, not diff contents.
pub fn change_signature(root: &Path) -> Result<u64> {
    use std::hash::{Hash, Hasher};
    use std::os::unix::fs::MetadataExt;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let status = status(root)?;
    status.hash(&mut hash);
    git(root, &args(&["rev-parse", "HEAD"]))
        .ok()
        .hash(&mut hash);
    let index = git(root, &args(&["rev-parse", "--git-path", "index"]))?;
    let index = PathBuf::from(OsString::from_vec(
        index.strip_suffix(b"\n").unwrap_or(&index).to_vec(),
    ));
    let mut paths = vec![root.join(index)];
    paths.extend(
        records_from_status(root, &status)?
            .into_iter()
            .map(|r| root.join(r.change.path)),
    );
    for path in paths {
        path.hash(&mut hash);
        if let Ok(meta) = fs::symlink_metadata(path) {
            (
                meta.len(),
                meta.mtime(),
                meta.mtime_nsec(),
                meta.ctime(),
                meta.ctime_nsec(),
            )
                .hash(&mut hash);
        }
    }
    Ok(hash.finish())
}

/// Branch label also works for unborn branches and detached HEADs.
pub fn branch_name(root: &Path) -> Result<String> {
    let name = git(root, &args(&["symbolic-ref", "--quiet", "--short", "HEAD"]))
        .or_else(|_| git(root, &args(&["rev-parse", "--short", "HEAD"])))?;
    Ok(String::from_utf8_lossy(&name).trim().to_owned())
}

pub fn diff(root: &Path, relative: &Path, scope: ChangeScope) -> Result<String> {
    diff_with_context(root, relative, scope, 3)
}

/// Bounded full context lets the UI expand unchanged lines without rereading disk.
pub fn diff_with_context(
    root: &Path,
    relative: &Path,
    scope: ChangeScope,
    context: usize,
) -> Result<String> {
    let canonical_root = root.canonicalize()?;
    let root = canonical_root.as_path();
    valid_relative(relative)?;
    if scope == ChangeScope::Untracked {
        return match read_file(root, relative)? {
            FileContent::Text(text) => {
                let mut patch = format!(
                    "--- /dev/null\n+++ {}\n@@ -0,0 +1,{} @@\n",
                    relative.display(),
                    text.lines().count()
                );
                for line in text.lines() {
                    patch.push('+');
                    patch.push_str(line);
                    patch.push('\n');
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    patch.push_str("\\ No newline at end of file\n");
                }
                Ok(patch)
            }
            _ => Ok("Binary or oversized file; text diff unavailable.".into()),
        };
    }
    // Resolve an existing path (or the deleted file's parent) before Git reads it.
    if root.join(relative).symlink_metadata().is_ok() {
        resolve(root, relative)?;
    } else {
        let mut parent = relative.parent().unwrap_or(Path::new(""));
        while !root.join(parent).try_exists()? {
            parent = parent
                .parent()
                .context("Working directory is unavailable")?;
        }
        resolve(root, parent)?;
    }
    let mut command = args(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--find-renames",
    ]);
    command.push(format!("--unified={}", context.min(MAX_TEXT_LINES)).into());
    if scope == ChangeScope::Staged {
        command.push("--cached".into());
    }
    command.push("--".into());
    command.push(relative.as_os_str().to_owned());
    if let Some(old) = records(root)?
        .into_iter()
        .find(|record| record.change.path == relative && record.change.scope == scope)
        .and_then(|record| record.old_path)
    {
        valid_relative(&old)?;
        command.push(old.into_os_string());
    }
    let patch = String::from_utf8_lossy(&git(root, &command)?).into_owned();
    if !supported_text(&patch) {
        bail!("Diff exceeds the text preview line limit")
    }
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "yes-preview-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn git(&self, values: &[&str]) {
            let output = Command::new("git")
                .current_dir(&self.0)
                .args(values)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn branch_labels_and_expanded_diff_respect_git_scope() {
        let f = Fixture::new();
        f.git(&["init", "-q", "-b", "preview-test"]);
        assert_eq!(branch_name(&f.0).unwrap(), "preview-test");
        f.git(&["config", "user.email", "test@example.com"]);
        f.git(&["config", "user.name", "Test"]);
        let original = (0..30).map(|i| format!("line {i}\n")).collect::<String>();
        fs::write(f.0.join("file.txt"), &original).unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-qm", "initial"]);
        let staged = original.replace("line 15\n", "staged\n");
        fs::write(f.0.join("file.txt"), &staged).unwrap();
        f.git(&["add", "."]);
        fs::write(f.0.join("file.txt"), staged.replace("staged", "working")).unwrap();
        let patch =
            diff_with_context(&f.0, Path::new("file.txt"), ChangeScope::Staged, 100_000).unwrap();
        assert!(patch.contains(" line 0\n") && patch.contains(" line 29\n"));
        assert!(patch.contains("+staged") && !patch.contains("+working"));
        let compact = diff(&f.0, Path::new("file.txt"), ChangeScope::Staged).unwrap();
        assert!(!compact.contains(" line 0\n"));
        f.git(&["checkout", "--detach", "-q"]);
        let detached = branch_name(&f.0).unwrap();
        assert!(detached.len() >= 7 && detached.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn image_versions_follow_index_worktree_and_rename() {
        let f = Fixture::new();
        f.git(&["init", "-q"]);
        f.git(&["config", "user.email", "test@example.com"]);
        f.git(&["config", "user.name", "Test"]);
        let old = b"<svg>old</svg>";
        let staged = b"<svg>staged</svg>";
        let current = b"<svg>current</svg>";
        fs::write(f.0.join("image.svg"), old).unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-qm", "initial"]);
        fs::write(f.0.join("image.svg"), staged).unwrap();
        f.git(&["add", "."]);
        fs::write(f.0.join("image.svg"), current).unwrap();
        let bytes = |side: Option<FileContent>| match side {
            Some(FileContent::Image { bytes, .. }) => bytes,
            _ => panic!("expected image"),
        };
        let (a, b) = image_diff(&f.0, Path::new("image.svg"), ChangeScope::Staged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(a), old);
        assert_eq!(bytes(b), staged);
        let (a, b) = image_diff(&f.0, Path::new("image.svg"), ChangeScope::Unstaged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(a), staged);
        assert_eq!(bytes(b), current);
        fs::remove_file(f.0.join("image.svg")).unwrap();
        let (a, b) = image_diff(&f.0, Path::new("image.svg"), ChangeScope::Unstaged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(a), staged);
        assert!(b.is_none());
        fs::write(f.0.join("new.svg"), current).unwrap();
        let (a, b) = image_diff(&f.0, Path::new("new.svg"), ChangeScope::Untracked)
            .unwrap()
            .unwrap();
        assert!(a.is_none());
        assert_eq!(bytes(b), current);
        let png = b"\x89PNG\r\n\x1a\nimage";
        fs::write(f.0.join("asset"), png).unwrap();
        f.git(&["add", "asset"]);
        fs::write(f.0.join("asset"), "working tree text").unwrap();
        let (_, b) = image_diff(&f.0, Path::new("asset"), ChangeScope::Staged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(b), png);
        fs::remove_file(f.0.join("asset")).unwrap();
        let (a, b) = image_diff(&f.0, Path::new("asset"), ChangeScope::Unstaged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(a), png);
        assert!(b.is_none());
        f.git(&["reset", "--hard", "-q", "HEAD"]);
        f.git(&["mv", "image.svg", "renamed.svg"]);
        let (a, b) = image_diff(&f.0, Path::new("renamed.svg"), ChangeScope::Staged)
            .unwrap()
            .unwrap();
        assert_eq!(bytes(a), old);
        assert_eq!(bytes(b), old);
        assert!(image_diff(&f.0, Path::new("../outside.svg"), ChangeScope::Staged).is_err());
    }

    #[test]
    fn image_probe_handles_text_magic_and_literal_revision_paths() {
        let f = Fixture::new();
        f.git(&["init", "-q"]);
        f.git(&["config", "user.email", "test@example.com"]);
        f.git(&["config", "user.name", "Test"]);
        let path = Path::new("asset\nwithout-extension");
        fs::write(f.0.join(path), "original text").unwrap();
        fs::write(f.0.join("empty.png"), []).unwrap();
        f.git(&["add", "."]);
        f.git(&["commit", "-qm", "initial"]);
        let png = b"\x89PNG\r\n\x1a\nnew image bytes";
        fs::write(f.0.join(path), png).unwrap();
        let (before, after) = image_diff(&f.0, path, ChangeScope::Unstaged)
            .unwrap()
            .unwrap();
        assert!(matches!(before, Some(FileContent::Unsupported)));
        assert!(matches!(after, Some(FileContent::Image { bytes, .. }) if bytes == png));
        f.git(&["add", "."]);
        fs::write(f.0.join(path), "working tree is text again").unwrap();
        let (before, after) = image_diff(&f.0, path, ChangeScope::Unstaged)
            .unwrap()
            .unwrap();
        assert!(matches!(before, Some(FileContent::Image { bytes, .. }) if bytes == png));
        assert!(matches!(after, Some(FileContent::Unsupported)));
        let empty = revision_image(&f.0, "HEAD:empty.png".into(), Path::new("empty.png")).unwrap();
        assert!(matches!(empty, Some(FileContent::Image { bytes, .. }) if bytes.is_empty()));
        assert!(
            revision_image(&f.0, "HEAD:missing.png".into(), Path::new("missing.png"))
                .unwrap()
                .is_none()
        );
        // A large ordinary text file stays on the text-diff path without loading
        // either complete version just to decide whether it is an image.
        fs::write(
            f.0.join("large.txt"),
            "x".repeat(MAX_FILE_BYTES as usize + 1),
        )
        .unwrap();
        f.git(&["add", "large.txt"]);
        assert!(
            image_diff(&f.0, Path::new("large.txt"), ChangeScope::Staged)
                .unwrap()
                .is_none()
        );
        std::os::unix::fs::symlink("/etc/passwd", f.0.join("escape.png")).unwrap();
        assert!(image_diff(&f.0, Path::new("escape.png"), ChangeScope::Untracked).is_err());
    }

    #[test]
    fn gitlink_changes_fall_back_to_text_diff_without_opening_the_directory() {
        let f = Fixture::new();
        f.git(&["init", "-q"]);
        f.git(&["config", "user.email", "test@example.com"]);
        f.git(&["config", "user.name", "Test"]);
        let module = Fixture(f.0.join("module"));
        fs::create_dir(&module.0).unwrap();
        module.git(&["init", "-q"]);
        module.git(&["config", "user.email", "test@example.com"]);
        module.git(&["config", "user.name", "Test"]);
        fs::write(module.0.join("file.txt"), "one").unwrap();
        module.git(&["add", "."]);
        module.git(&["commit", "-qm", "initial"]);
        f.git(&["add", "module"]);
        f.git(&["commit", "-qm", "track gitlink"]);
        fs::write(module.0.join("file.txt"), "two").unwrap();
        module.git(&["commit", "-qam", "update"]);
        let path = Path::new("module");
        assert!(
            image_diff(&f.0, path, ChangeScope::Unstaged)
                .unwrap()
                .is_none()
        );
        let patch = diff(&f.0, path, ChangeScope::Unstaged).unwrap();
        assert!(patch.contains("-Subproject commit "));
        assert!(patch.contains("+Subproject commit "));
        f.git(&["add", "module"]);
        assert!(
            image_diff(&f.0, path, ChangeScope::Staged)
                .unwrap()
                .is_none()
        );
        assert!(
            diff(&f.0, path, ChangeScope::Staged)
                .unwrap()
                .contains("+Subproject commit ")
        );
    }

    #[test]
    fn change_signal_uses_subdirectory_paths_from_the_same_status_snapshot() {
        let f = Fixture::new();
        f.git(&["init", "-q"]);
        fs::create_dir(f.0.join("sub")).unwrap();
        fs::write(f.0.join("sub/file.txt"), "one").unwrap();
        fs::write(f.0.join("outside.txt"), "one").unwrap();
        let root = f.0.join("sub");
        let before = change_signature(&root).unwrap();
        fs::write(f.0.join("outside.txt"), "outside update").unwrap();
        assert_eq!(before, change_signature(&root).unwrap());
        fs::write(root.join("file.txt"), "inside update").unwrap();
        assert_ne!(before, change_signature(&root).unwrap());
    }

    #[test]
    fn change_signal_detects_edits_even_when_status_is_unchanged() {
        let f = Fixture::new();
        f.git(&["init", "-q"]);
        fs::write(f.0.join("file.txt"), "one").unwrap();
        let first = change_signature(&f.0).unwrap();
        assert_eq!(first, change_signature(&f.0).unwrap());
        fs::write(f.0.join("file.txt"), "different content").unwrap();
        let second = change_signature(&f.0).unwrap();
        assert_ne!(first, second);
        f.git(&["add", "."]);
        assert_ne!(second, change_signature(&f.0).unwrap());
    }

    #[test]
    fn loaded_entries_follow_gitignore_rules_without_hiding_files() {
        let fixture = Fixture::new();
        fixture.git(&["init", "-q"]);
        fs::create_dir_all(fixture.0.join("build")).unwrap();
        fs::create_dir_all(fixture.0.join("src")).unwrap();
        fs::write(fixture.0.join(".gitignore"), "build/\n*.log\n!keep.log\n").unwrap();
        fs::write(fixture.0.join("src/.gitignore"), "*.tmp\n!keep.tmp\n").unwrap();
        for path in [
            "ignored.log",
            "keep.log",
            "tracked.log",
            "build/output.txt",
            "src/隐藏.tmp",
            "src/keep.tmp",
            "src/newline\n.tmp",
        ] {
            fs::write(fixture.0.join(path), "text").unwrap();
        }
        fixture.git(&["add", "-f", "tracked.log"]);
        let loaded = entries(&fixture.0, Path::new("")).unwrap();
        for (path, ignored) in [
            ("build", true),
            ("ignored.log", true),
            ("keep.log", false),
            ("tracked.log", false),
            ("src", false),
        ] {
            assert_eq!(
                loaded
                    .iter()
                    .find(|entry| entry.path == Path::new(path))
                    .unwrap()
                    .ignored,
                ignored,
                "{path}"
            );
        }
        let nested = entries(&fixture.0, Path::new("src")).unwrap();
        assert!(
            nested
                .iter()
                .find(|entry| entry.path == Path::new("src/隐藏.tmp"))
                .unwrap()
                .ignored
        );
        assert!(
            !nested
                .iter()
                .find(|entry| entry.path == Path::new("src/keep.tmp"))
                .unwrap()
                .ignored
        );
        assert!(
            nested
                .iter()
                .find(|entry| entry.path == Path::new("src/newline\n.tmp"))
                .unwrap()
                .ignored
        );
        assert!(entries(&fixture.0, Path::new("build")).unwrap()[0].ignored);
        assert!(search(&fixture.0, "隐藏").unwrap()[0].ignored);
        assert!(
            entries(&fixture.0.join("src"), Path::new(""))
                .unwrap()
                .iter()
                .any(|entry| entry.ignored)
        );
    }

    #[test]
    fn images_and_unsupported_documents_are_classified_before_text() {
        let fixture = Fixture::new();
        for (name, bytes, expected) in [
            (
                "png.data",
                b"\x89PNG\r\n\x1a\n".as_slice(),
                PreviewImageFormat::Png,
            ),
            (
                "jpg.data",
                b"\xff\xd8\xff".as_slice(),
                PreviewImageFormat::Jpeg,
            ),
            ("gif.data", b"GIF89a".as_slice(), PreviewImageFormat::Gif),
            (
                "webp.data",
                b"RIFFxxxxWEBP".as_slice(),
                PreviewImageFormat::Webp,
            ),
            ("bmp.data", b"BM".as_slice(), PreviewImageFormat::Bmp),
            ("tiff.data", b"II*\0".as_slice(), PreviewImageFormat::Tiff),
            (
                "ico.data",
                b"\0\0\x01\0".as_slice(),
                PreviewImageFormat::Ico,
            ),
            (
                "image.SVG",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".as_slice(),
                PreviewImageFormat::Svg,
            ),
            (
                "broken.PNG",
                b"damaged image".as_slice(),
                PreviewImageFormat::Png,
            ),
        ] {
            fs::write(fixture.0.join(name), bytes).unwrap();
            assert!(
                matches!(read_file(&fixture.0, Path::new(name)).unwrap(), FileContent::Image { format, .. } if format == expected)
            );
        }
        for (name, content) in [
            ("document.pdf", "%PDF-1.7\nASCII contents"),
            ("archive.zip", "PK archive"),
            ("video.mp4", "video"),
            ("control.data", "\u{1}binary"),
        ] {
            fs::write(fixture.0.join(name), content).unwrap();
            assert!(matches!(
                read_file(&fixture.0, Path::new(name)).unwrap(),
                FileContent::Unsupported
            ));
        }
    }

    #[test]
    fn file_boundaries_and_formats() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        fs::write(outside.0.join("private"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("escape")).unwrap();
        fs::write(fixture.0.join("hello 中文.txt"), "hello").unwrap();
        fs::write(fixture.0.join("binary"), b"a\0b").unwrap();
        let large = fs::File::create(fixture.0.join("large")).unwrap();
        large.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(read_file(&fixture.0, Path::new("../private")).is_err());
        assert!(read_file(&fixture.0, &outside.0.join("private")).is_err());
        assert!(read_file(&fixture.0, Path::new("escape/private")).is_err());
        assert!(
            matches!(read_file(&fixture.0, Path::new("hello 中文.txt")).unwrap(), FileContent::Text(text) if text == "hello")
        );
        assert!(matches!(
            read_file(&fixture.0, Path::new("binary")).unwrap(),
            FileContent::Unsupported
        ));
        assert!(matches!(
            read_file(&fixture.0, Path::new("large")).unwrap(),
            FileContent::Unsupported
        ));
        assert!(
            !entries(&fixture.0, Path::new(""))
                .unwrap()
                .iter()
                .any(|entry| entry.path == Path::new("escape"))
        );
        assert_eq!(search(&fixture.0, "中文").unwrap().len(), 1);
    }

    #[test]
    fn text_and_diff_layout_limits() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("minified"), "x".repeat(MAX_LINE_BYTES + 1)).unwrap();
        fs::write(
            fixture.0.join("many-lines"),
            "x\n".repeat(MAX_TEXT_LINES + 1),
        )
        .unwrap();
        for path in ["minified", "many-lines"] {
            assert!(matches!(
                read_file(&fixture.0, Path::new(path)).unwrap(),
                FileContent::Unsupported
            ));
        }
        fixture.git(&["init", "-q"]);
        fs::write(fixture.0.join("binary"), b"a\0b").unwrap();
        fixture.git(&["add", "minified", "binary"]);
        let changes = changes(&fixture.0).unwrap();
        let binary = changes
            .iter()
            .find(|change| change.path == Path::new("binary"))
            .unwrap();
        assert_eq!((binary.additions, binary.deletions), (None, None));
        assert!(diff(&fixture.0, Path::new("minified"), ChangeScope::Staged).is_err());
    }

    #[test]
    fn nongit_unborn_head_and_literal_paths() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("plain.txt"), "plain\n").unwrap();
        assert!(changes(&fixture.0).is_err());
        assert!(diff(&fixture.0, Path::new("plain.txt"), ChangeScope::Staged).is_err());
        fixture.git(&["init", "-q"]);
        fs::write(fixture.0.join("-leading.txt"), "leading\n").unwrap();
        fs::write(fixture.0.join(":(glob)*"), "literal\n").unwrap();
        fixture.git(&[
            "--literal-pathspecs",
            "add",
            "--",
            "-leading.txt",
            ":(glob)*",
        ]);
        let list = changes(&fixture.0).unwrap();
        assert_eq!(list.len(), 3);
        assert!(
            list.iter()
                .filter(|change| change.scope == ChangeScope::Staged)
                .all(|change| change.additions == Some(1) && change.deletions == Some(0))
        );
        assert!(
            list.iter()
                .any(|c| c.path == Path::new("plain.txt") && c.scope == ChangeScope::Untracked)
        );
        let leading = diff(&fixture.0, Path::new("-leading.txt"), ChangeScope::Staged).unwrap();
        assert!(leading.contains("+leading"));
        assert!(!leading.contains("+literal"));
        let literal = diff(&fixture.0, Path::new(":(glob)*"), ChangeScope::Staged).unwrap();
        assert!(literal.contains("+literal"));
        assert!(!literal.contains("+leading"));
    }

    #[test]
    fn renamed_file_unstaged_diff_excludes_recreated_source() {
        let fixture = Fixture::new();
        fixture.git(&["init", "-q"]);
        fixture.git(&["config", "user.name", "Preview Test"]);
        fixture.git(&["config", "user.email", "preview@example.test"]);
        fs::create_dir(fixture.0.join("deleted-dir")).unwrap();
        fs::write(fixture.0.join("deleted-dir/file"), "deleted\n").unwrap();
        fs::write(fixture.0.join("old"), "original\n").unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-qm", "fixture"]);
        fixture.git(&["mv", "old", "new"]);
        fs::write(fixture.0.join("old"), "recreated source\n").unwrap();
        fixture.git(&["add", "old"]);
        fs::write(fixture.0.join("old"), "unrelated edit\n").unwrap();
        fs::write(fixture.0.join("new"), "new edit\n").unwrap();
        fs::remove_dir_all(fixture.0.join("deleted-dir")).unwrap();
        let patch = diff(&fixture.0, Path::new("new"), ChangeScope::Unstaged).unwrap();
        assert!(patch.contains("+new edit"));
        assert!(!patch.contains("unrelated edit"));
        let deletion = diff(
            &fixture.0,
            Path::new("deleted-dir/file"),
            ChangeScope::Unstaged,
        )
        .unwrap();
        assert!(deletion.contains("-deleted"));
    }

    #[test]
    fn git_scopes_renames_deletions_subdirectories_and_worktrees() {
        let fixture = Fixture::new();
        fixture.git(&["init", "-q"]);
        fixture.git(&["config", "user.name", "Preview Test"]);
        fixture.git(&["config", "user.email", "preview@example.test"]);
        fs::create_dir(fixture.0.join("sub")).unwrap();
        for path in ["both.txt", "remove.txt", "old 中文.txt", "sub/nested.txt"] {
            fs::write(fixture.0.join(path), "original\n").unwrap();
        }
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-qm", "fixture"]);
        fs::write(fixture.0.join("both.txt"), "staged\n").unwrap();
        fixture.git(&["add", "both.txt"]);
        fs::write(fixture.0.join("both.txt"), "working\n").unwrap();
        fs::remove_file(fixture.0.join("remove.txt")).unwrap();
        fixture.git(&["mv", "old 中文.txt", "new 中文.txt"]);
        fs::write(fixture.0.join("new\nfile.txt"), "new\n").unwrap();
        fs::write(fixture.0.join("sub/nested.txt"), "changed\n").unwrap();
        let list = changes(&fixture.0).unwrap();
        for change in list
            .iter()
            .filter(|change| change.path == Path::new("both.txt"))
        {
            assert_eq!((change.additions, change.deletions), (Some(1), Some(1)));
        }
        let renamed = list
            .iter()
            .find(|change| change.path == Path::new("new 中文.txt"))
            .unwrap();
        assert_eq!((renamed.additions, renamed.deletions), (Some(0), Some(0)));
        let deleted = list
            .iter()
            .find(|change| change.path == Path::new("remove.txt"))
            .unwrap();
        assert_eq!((deleted.additions, deleted.deletions), (Some(0), Some(1)));
        assert_eq!(
            list.iter()
                .filter(|change| change.path == Path::new("both.txt"))
                .count(),
            2
        );
        assert!(
            list.iter()
                .any(|change| change.path == Path::new("new\nfile.txt")
                    && change.scope == ChangeScope::Untracked)
        );
        assert!(
            diff(&fixture.0, Path::new("both.txt"), ChangeScope::Staged)
                .unwrap()
                .contains("+staged")
        );
        assert!(
            diff(&fixture.0, Path::new("both.txt"), ChangeScope::Unstaged)
                .unwrap()
                .contains("+working")
        );
        assert!(
            diff(&fixture.0, Path::new("remove.txt"), ChangeScope::Unstaged)
                .unwrap()
                .contains("-original")
        );
        assert!(
            diff(&fixture.0, Path::new("new 中文.txt"), ChangeScope::Staged)
                .unwrap()
                .contains("rename from")
        );
        assert!(
            diff(
                &fixture.0,
                Path::new("new\nfile.txt"),
                ChangeScope::Untracked
            )
            .unwrap()
            .contains("+new")
        );
        let nested = changes(&fixture.0.join("sub")).unwrap();
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].path, Path::new("nested.txt"));
        assert_eq!(
            (nested[0].additions, nested[0].deletions),
            (Some(1), Some(1))
        );
        assert!(
            diff(
                &fixture.0.join("sub"),
                Path::new("nested.txt"),
                ChangeScope::Unstaged
            )
            .unwrap()
            .contains("+changed")
        );
        let worktree = Fixture::new();
        fixture.git(&[
            "worktree",
            "add",
            "--detach",
            worktree.0.to_str().unwrap(),
            "HEAD",
        ]);
        fs::write(worktree.0.join("both.txt"), "worktree\n").unwrap();
        assert_eq!(changes(&worktree.0).unwrap().len(), 1);
    }
}

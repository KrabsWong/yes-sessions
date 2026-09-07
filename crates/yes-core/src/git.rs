use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileChange {
    pub path: PathBuf,
    pub status: GitFileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub staged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitStatus {
    pub is_git_repo: bool,
    pub branch: String,
    pub ahead: usize,
    pub behind: usize,
    pub changes: Vec<GitFileChange>,
}

fn git(directory: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .with_context(|| format!("run git in {}", directory.display()))?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "))
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn diff_stats(directory: &Path, path: &str, staged: bool) -> (usize, usize) {
    let mut args = vec!["diff"];
    if staged {
        args.push("--staged")
    }
    args.extend(["--numstat", "--", path]);
    git(directory, &args)
        .ok()
        .and_then(|output| {
            let mut fields = output.lines().next()?.split('\t');
            Some((
                fields.next()?.parse().unwrap_or(0),
                fields.next()?.parse().unwrap_or(0),
            ))
        })
        .unwrap_or_default()
}

pub fn status(directory: &Path) -> Result<GitStatus> {
    if !directory.join(".git").exists() || git(directory, &["rev-parse", "--git-dir"]).is_err() {
        return Ok(GitStatus::default());
    }
    let branch = git(directory, &["rev-parse", "--abbrev-ref", "HEAD"])
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (ahead, behind) = git(
        directory,
        &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
    )
    .ok()
    .and_then(|output| {
        let mut values = output.split_whitespace();
        Some((values.next()?.parse().ok()?, values.next()?.parse().ok()?))
    })
    .unwrap_or_default();
    let output = git(directory, &["status", "--porcelain", "-uall"])?;
    let mut changes = Vec::new();
    for line in output.lines().filter(|line| line.len() >= 4) {
        let bytes = line.as_bytes();
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let path = &line[3..];
        let staged = index != ' ' && index != '?';
        let code = if staged { index } else { worktree };
        let file_status = match (index, worktree, code) {
            ('?', '?', _) => GitFileStatus::Untracked,
            (_, _, 'A') => GitFileStatus::Added,
            (_, _, 'M') => GitFileStatus::Modified,
            (_, _, 'D') => GitFileStatus::Deleted,
            (_, _, 'R') => GitFileStatus::Renamed,
            _ => GitFileStatus::Unknown,
        };
        let (additions, deletions) = if file_status == GitFileStatus::Untracked {
            (
                std::fs::read_to_string(directory.join(path))
                    .map(|source| source.lines().count())
                    .unwrap_or(0),
                0,
            )
        } else {
            diff_stats(directory, path, staged)
        };
        changes.push(GitFileChange {
            path: PathBuf::from(path),
            status: file_status,
            additions,
            deletions,
            staged,
        });
    }
    Ok(GitStatus {
        is_git_repo: true,
        branch,
        ahead,
        behind,
        changes,
    })
}

pub fn diff(directory: &Path, file: Option<&Path>) -> Result<String> {
    if let Some(file) = file {
        if file.is_absolute()
            || file
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            bail!("invalid git path")
        }
        git(directory, &["diff", "--", &file.to_string_lossy()])
    } else {
        git(directory, &["diff"])
    }
}

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

use crate::{AppType, settings::PreferredTerminal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExternalTerminal {
    Ghostty,
    Kitty,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInfo {
    pub preferred: ExternalTerminal,
    pub ghostty_installed: bool,
    pub kitty_installed: bool,
}

fn command_exists(command: &str) -> bool {
    Command::new("/usr/bin/which")
        .arg(command)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn kitty_path() -> Option<PathBuf> {
    if command_exists("kitty") {
        return Some(PathBuf::from("kitty"));
    }
    let candidates = [
        PathBuf::from("/Applications/kitty.app/Contents/MacOS/kitty"),
        dirs::home_dir()
            .unwrap_or_default()
            .join("Applications/kitty.app/Contents/MacOS/kitty"),
    ];
    candidates.into_iter().find(|path| path.exists())
}

pub fn terminal_info(preference: PreferredTerminal) -> TerminalInfo {
    let ghostty_installed =
        command_exists("ghostty") || Path::new("/Applications/Ghostty.app").exists();
    let kitty_installed = kitty_path().is_some();
    let preferred = match preference {
        PreferredTerminal::Ghostty if ghostty_installed => ExternalTerminal::Ghostty,
        PreferredTerminal::Kitty if kitty_installed => ExternalTerminal::Kitty,
        PreferredTerminal::Terminal => ExternalTerminal::Terminal,
        _ if ghostty_installed => ExternalTerminal::Ghostty,
        _ if kitty_installed => ExternalTerminal::Kitty,
        _ => ExternalTerminal::Terminal,
    };
    TerminalInfo {
        preferred,
        ghostty_installed,
        kitty_installed,
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn resume_command(app_type: AppType, session_id: &str) -> (&'static str, Vec<String>) {
    match app_type {
        AppType::Claude => ("claude", vec![format!("--resume={session_id}")]),
        AppType::OpenCode => ("opencode", vec!["-s".into(), session_id.into()]),
        AppType::CodeBuddy => ("codebuddy", vec![format!("--resume={session_id}")]),
        AppType::Codex => ("codex", vec!["resume".into(), session_id.into()]),
    }
}

pub fn resume_session(
    app_type: AppType,
    session_id: &str,
    working_dir: Option<&Path>,
    preference: PreferredTerminal,
) -> io::Result<()> {
    let terminal = terminal_info(preference).preferred;
    let (command, args) = resume_command(app_type, session_id);
    let command_line = std::iter::once(shell_quote(command))
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    let working_dir = working_dir.filter(|path| path.is_dir());
    let shell_command = working_dir
        .map(|path| {
            format!(
                "cd {} && {command_line}",
                shell_quote(&path.to_string_lossy())
            )
        })
        .unwrap_or(command_line);

    let mut process = match terminal {
        ExternalTerminal::Ghostty => {
            let mut process = Command::new("ghostty");
            process.args(["-e", "zsh", "-ic", &format!("{shell_command}; exec zsh -i")]);
            process
        }
        ExternalTerminal::Kitty => {
            let mut process = Command::new(kitty_path().unwrap_or_else(|| PathBuf::from("kitty")));
            if let Some(path) = working_dir {
                process.args(["--working-directory", &path.to_string_lossy()]);
            }
            process.args(["-e", "zsh", "-ic", &shell_command]);
            process
        }
        ExternalTerminal::Terminal => {
            let escaped = shell_command.replace('\\', "\\\\").replace('"', "\\\"");
            let script = format!(
                "tell application \"Terminal\"\nif not (exists window 1) then\ndo script \"{escaped}\"\nelse\ndo script \"{escaped}\" in window 1\nend if\nactivate\nend tell"
            );
            let mut process = Command::new("osascript");
            process.args(["-e", &script]);
            process
        }
    };
    process
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_shell_arguments() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("don't"), "'don'\\''t'");
    }

    #[test]
    fn builds_provider_resume_commands() {
        assert_eq!(
            resume_command(AppType::Codex, "abc"),
            ("codex", vec!["resume".to_owned(), "abc".to_owned()])
        );
        assert_eq!(
            resume_command(AppType::Claude, "abc"),
            ("claude", vec!["--resume=abc".to_owned()])
        );
    }
}

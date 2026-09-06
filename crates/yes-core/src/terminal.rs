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

fn terminal_path(command: &str, bundle: &str) -> Option<PathBuf> {
    let home = dirs::home_dir();
    resolve_terminal_path(
        command,
        bundle,
        Path::new("/Applications"),
        home.as_deref(),
        command_exists(command),
    )
}

fn resolve_terminal_path(
    command: &str,
    bundle: &str,
    applications: &Path,
    home: Option<&Path>,
    on_path: bool,
) -> Option<PathBuf> {
    if on_path {
        return Some(PathBuf::from(command));
    }
    let executable = PathBuf::from(bundle).join("Contents/MacOS").join(command);
    std::iter::once(applications.join(&executable))
        .chain(home.map(|home| home.join("Applications").join(&executable)))
        .find(|path| {
            use std::os::unix::fs::PermissionsExt as _;
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn ghostty_path() -> Option<PathBuf> {
    terminal_path("ghostty", "Ghostty.app")
}

fn kitty_path() -> Option<PathBuf> {
    terminal_path("kitty", "kitty.app")
}

pub fn terminal_info(preference: PreferredTerminal) -> TerminalInfo {
    let ghostty_installed = ghostty_path().is_some();
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
            let path = ghostty_path().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Ghostty executable was not found")
            })?;
            let mut process = Command::new(path);
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
    fn resolves_installed_ghostty_without_a_path_entry() {
        use std::{fs, os::unix::fs::PermissionsExt as _};
        let root = std::env::temp_dir().join(format!("yes-ghostty-test-{}", std::process::id()));
        let applications = root.join("Applications");
        let home = root.join("home");
        let relative = "Ghostty.app/Contents/MacOS/ghostty";
        let executable = applications.join(relative);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve_terminal_path("ghostty", "Ghostty.app", &applications, Some(&home), false),
            Some(executable.clone())
        );
        let user_executable = home.join("Applications").join(relative);
        fs::create_dir_all(user_executable.parent().unwrap()).unwrap();
        fs::rename(&executable, &user_executable).unwrap();
        assert_eq!(
            resolve_terminal_path("ghostty", "Ghostty.app", &applications, Some(&home), false),
            Some(user_executable.clone())
        );
        fs::set_permissions(&user_executable, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            resolve_terminal_path("ghostty", "Ghostty.app", &applications, Some(&home), false),
            None
        );
        assert_eq!(
            resolve_terminal_path("ghostty", "Ghostty.app", &applications, Some(&home), true),
            Some(PathBuf::from("ghostty"))
        );
        fs::remove_dir_all(root).unwrap();
    }

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

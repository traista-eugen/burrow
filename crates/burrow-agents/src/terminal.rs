//! The commands that run each agent's own TUI, for terminal sessions (M4, D-038).
//!
//! A terminal session runs the agent's interactive TUI on a pseudo console
//! ([`crate::launcher::spawn_pty`]). Each adapter's [`AgentAdapter::terminal`] says what to run:
//!
//! | Agent | Runs | Instead of |
//! |---|---|---|
//! | Claude Code | `node_modules\@anthropic-ai\claude-code\bin\claude.exe`, next to npm's `claude.cmd` | the `claude.cmd` shim |
//! | Codex | `node node_modules\@openai\codex\bin\codex.js`, next to npm's `codex.cmd` | the `codex.cmd` shim |
//! | Cursor | `versions\<newest>\node.exe versions\<newest>\index.js`, as for ACP (D-020) | `agent.cmd`, which starts PowerShell |
//! | Muse | `muse-bin-<version>.exe` (D-023) | `muse.cmd`, which starts PowerShell |
//!
//! Each runs what its shim would run, so no `cmd.exe` or PowerShell sits between the console
//! and the TUI: a `cmd.exe` running a batch file asks "Terminate batch job (Y/N)?" after a
//! Ctrl+C, inside the TUI's screen. When the layout isn't the expected one, the shim runs
//! anyway, through `cmd.exe`.
//!
//! The environment is `burrowd`'s, without the variables that name the terminal `burrowd` was
//! started from (Windows Terminal's `WT_SESSION`, `TERM_PROGRAM`, …), `NO_COLOR`, and Claude
//! Code's own session markers, and with `TERM` and `COLORTERM` saying what Burrow's emulator
//! supports.
//!
//! [`AgentAdapter::terminal`]: crate::AgentAdapter::terminal

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::adapter::{AgentError, SessionConfig};
use crate::detect::{self, CliAgent};
use crate::launcher::LaunchSpec;

/// Variables `burrowd` may have inherited that a TUI in Burrow's terminal must not see:
/// - those that describe the terminal `burrowd` itself was started from;
/// - `NO_COLOR`, which a parent such as Claude Code sets for its own tools, and which would turn
///   every TUI's colours off;
/// - Claude Code's markers of a running session, with which a `claude` started under a
///   `burrowd` that a Claude Code session launched refuses to start ("nested session").
const FOREIGN_TERMINAL_ENV: &[&str] = &[
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "ConEmuANSI",
    "ConEmuPID",
    "VSCODE_INJECTION",
    "NO_COLOR",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
];

/// Finishes a TUI's command for a terminal session: its folder, its label and its environment.
pub fn for_session(spec: LaunchSpec, config: &SessionConfig) -> LaunchSpec {
    let mut spec = spec.cwd(plain(&config.cwd));
    spec.label = format!("{} {}", spec.label, config.session_id);
    for name in FOREIGN_TERMINAL_ENV {
        spec = spec.env_remove(*name);
    }
    spec.env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
}

/// A canonical path without Windows' verbatim prefix: `\\?\C:\repo` as `C:\repo`. A TUI shows
/// its folder, and shells such as `cmd.exe` can't work in a verbatim one.
fn plain(path: &Path) -> PathBuf {
    match path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Claude Code's TUI: the native `claude.exe` npm's `claude.cmd` runs, else whatever
/// detection finds.
pub fn claude() -> Result<LaunchSpec, AgentError> {
    let found = find(&detect::CLAUDE)?;
    let program = shim_dir(&found)
        .map(|dir| dir.join("node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe"))
        .filter(|exe| exe.is_file())
        .unwrap_or(found);
    Ok(LaunchSpec::new(program, Vec::<OsString>::new(), "claude"))
}

/// Codex's TUI: `node codex.js`, as npm's `codex.cmd` runs it, else whatever detection finds.
/// `codex.js` starts the native `codex.exe` and passes the console on.
pub fn codex() -> Result<LaunchSpec, AgentError> {
    let found = find(&detect::CODEX)?;
    let shim = shim_dir(&found).and_then(|dir| {
        let script = dir.join("node_modules\\@openai\\codex\\bin\\codex.js");
        if !script.is_file() {
            return None;
        }
        // The shim prefers a node.exe beside it, as npm installs one on some setups.
        let node = Some(dir.join("node.exe"))
            .filter(|node| node.is_file())
            .or_else(crate::acp::install::find_node)?;
        Some(LaunchSpec::new(node, [script.into_os_string()], "codex"))
    });
    Ok(shim.unwrap_or_else(|| LaunchSpec::new(found, Vec::<OsString>::new(), "codex")))
}

/// Cursor's TUI: its bundled `node.exe` with `index.js`, as for ACP but without `acp`.
pub fn cursor(install_dir: Option<PathBuf>) -> Result<LaunchSpec, AgentError> {
    let dir = install_dir.ok_or_else(|| {
        AgentError::Unavailable("can't find the Cursor CLI's install folder".into())
    })?;
    let mut command = burrow_acp::launch::cursor_command(&dir).ok_or_else(|| {
        AgentError::Unavailable(format!(
            "can't find Cursor's bundled node.exe and index.js under {}",
            dir.join("versions").display()
        ))
    })?;
    // `cursor_command` runs `index.js acp`; the TUI is `index.js` alone.
    if command.args.last().is_some_and(|arg| arg == "acp") {
        command.args.pop();
    }
    let mut spec = LaunchSpec::new(command.program, command.args, "cursor");
    for (key, value) in command.env {
        spec = spec.env(key, value);
    }
    Ok(spec)
}

/// Muse's TUI: its versioned binary, with no arguments (D-023).
pub fn muse(binary: Option<PathBuf>) -> Result<LaunchSpec, AgentError> {
    let binary = binary.ok_or_else(|| {
        AgentError::Unavailable("Muse Code isn't installed: `muse` isn't on PATH".into())
    })?;
    Ok(LaunchSpec::new(binary, Vec::<OsString>::new(), "muse"))
}

fn find(agent: &CliAgent) -> Result<PathBuf, AgentError> {
    agent.find().ok_or_else(|| {
        AgentError::Unavailable(format!(
            "{} isn't installed: `{}` isn't on PATH",
            agent.name, agent.program
        ))
    })
}

/// The folder of an npm shim (`claude.cmd`), where npm puts the package's `node_modules`.
fn shim_dir(program: &Path) -> Option<&Path> {
    let is_cmd = program
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd"));
    is_cmd.then(|| program.parent()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_get_their_folder_and_a_clean_terminal_environment() {
        let config = SessionConfig {
            session_id: "ses_1".into(),
            cwd: PathBuf::from("D:\\repo"),
            model: None,
        };
        let spec = for_session(LaunchSpec::new("x.exe", ["a"], "claude"), &config);
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("D:\\repo")));
        assert_eq!(spec.label, "claude ses_1");
        assert!(spec.env_remove.iter().any(|name| name == "WT_SESSION"));
        assert!(
            spec.env
                .iter()
                .any(|(key, value)| key == "TERM" && value == "xterm-256color")
        );
    }

    #[test]
    fn folders_lose_the_verbatim_prefix() {
        assert_eq!(plain(Path::new(r"\\?\C:\repo")), Path::new(r"C:\repo"));
        assert_eq!(plain(Path::new(r"C:\repo")), Path::new(r"C:\repo"));
        // A UNC share keeps it: `\\?\UNC\server\share` has no shorter form here.
        assert_eq!(
            plain(Path::new(r"\\?\UNC\server\share")),
            Path::new(r"\\?\UNC\server\share")
        );
    }

    #[test]
    fn npm_shims_are_seen_through() {
        assert_eq!(
            shim_dir(Path::new("C:\\npm\\claude.cmd")),
            Some(Path::new("C:\\npm"))
        );
        assert_eq!(shim_dir(Path::new("C:\\bin\\claude.exe")), None);
    }
}

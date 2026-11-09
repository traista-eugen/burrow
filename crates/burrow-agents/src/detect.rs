//! Detecting the agent CLIs: where they are, their version, and whether they are logged in.
//!
//! Every command here is read-only and spends no quota:
//!
//! | Agent | Version | Login |
//! |---|---|---|
//! | Claude Code | `claude --version` | `claude auth status` (JSON with `loggedIn`) |
//! | Codex | `codex --version` | `codex login status` ("Logged in using ChatGPT") |
//! | Cursor | `agent --version` | `agent status` ("✓ Logged in as …") |
//! | Muse | `muse --version` | `muse usage` ("not logged in", or the allowance) |
//!
//! Commands run through the launcher, in their own Job Objects, with a timeout.
//!
//! No models come from here. Cursor's `agent models` lists ids its ACP sessions don't accept
//! (`gpt-5.3-codex-high` where a session offers `gpt-5.3-codex[reasoning=medium,fast=false]`),
//! so `burrowd` reports the models of an agent's latest session instead (D-032).
//!
//! Muse is detected by [`crate::msp::MspAdapter`], which runs the same commands on Muse's
//! versioned binary, not its `muse.cmd` launcher (D-023), and asks a throwaway
//! `muse serve --provider echo` for the models and whether the shell sandbox is set up.

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use burrow_protocol::{AgentId, AgentInfo, AgentLogin, Integration, LoginState, Timestamp};
use serde_json::Value;
use tracing::debug;

use crate::launcher::{self, Captured, LaunchSpec};

/// How long one detection command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// An agent CLI that Burrow detects.
#[derive(Clone, Copy, Debug)]
pub struct CliAgent {
    /// Which agent.
    pub id: &'static str,
    /// Its display name.
    pub name: &'static str,
    /// How Burrow talks to it.
    pub integration: &'static str,
    /// The command's name on `PATH`.
    pub program: &'static str,
    /// Where its installer puts it, relative to an environment variable, for when `burrowd`'s
    /// `PATH` predates the install.
    pub install_dirs: &'static [(&'static str, &'static str)],
}

/// Claude Code (`claude`, installed by npm).
pub const CLAUDE: CliAgent = CliAgent {
    id: "claude",
    name: "Claude Code",
    integration: "acp",
    program: "claude",
    install_dirs: &[("APPDATA", "npm")],
};

/// Codex (`codex`, installed by npm).
pub const CODEX: CliAgent = CliAgent {
    id: "codex",
    name: "Codex",
    integration: "acp",
    program: "codex",
    install_dirs: &[("APPDATA", "npm")],
};

/// The Cursor CLI (`agent`, installed under `%LOCALAPPDATA%\cursor-agent`).
pub const CURSOR: CliAgent = CliAgent {
    id: "cursor",
    name: "Cursor",
    integration: "acp",
    program: "agent",
    install_dirs: &[("LOCALAPPDATA", "cursor-agent")],
};

/// Muse Code (`muse`, installed under `%LOCALAPPDATA%\Programs\muse`).
pub const MUSE: CliAgent = CliAgent {
    id: "muse",
    name: "Muse Code",
    integration: "msp",
    program: "muse",
    install_dirs: &[("LOCALAPPDATA", "Programs\\muse")],
};

impl CliAgent {
    /// The agent's id.
    pub fn agent_id(&self) -> AgentId {
        AgentId::from(self.id)
    }

    /// Finds the CLI: on `PATH`, then in its install directories.
    pub fn find(&self) -> Option<PathBuf> {
        let extra: Vec<PathBuf> = self
            .install_dirs
            .iter()
            .filter_map(|(var, dir)| env::var_os(var).map(|base| PathBuf::from(base).join(dir)))
            .collect();
        launcher::find_program_in(self.program, &extra)
    }
}

/// Detects one CLI agent. `available` is true when it is installed and not logged out; an
/// adapter that can't start it yet overrides that.
pub async fn detect(agent: &CliAgent) -> AgentInfo {
    let mut info = AgentInfo {
        id: agent.agent_id(),
        name: agent.name.to_owned(),
        integration: Integration::from(agent.integration),
        installed: false,
        version: None,
        path: None,
        login: AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: None,
        },
        models: Vec::new(),
        agent_modes: Vec::new(),
        available: false,
        problem: None,
        adapter: None,
        default_agent_mode: None,
        detected_at: Timestamp::now(),
    };
    let Some(program) = agent.find() else {
        info.problem = Some(format!("`{}` isn't on PATH", agent.program));
        return info;
    };
    info.installed = true;
    info.path = Some(program.display().to_string());

    let run = |args: &'static [&'static str]| {
        let spec = LaunchSpec::new(
            &program,
            args.iter().copied(),
            format!("{} {}", agent.program, args.join(" ")),
        );
        async move { launcher::run_capture(&spec, COMMAND_TIMEOUT).await }
    };
    let (version, login) = tokio::join!(run(&["--version"]), run(login_args(agent.id)));

    match version {
        Ok(output) => info.version = parse_version(&output.text()),
        Err(error) => debug!(agent = agent.id, %error, "version check failed"),
    }
    info.login = match login {
        Ok(output) => parse_login(agent.id, &output),
        Err(error) => AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: Some(format!("checking the login failed: {error}")),
        },
    };
    match info.login.state {
        LoginState::LoggedOut => {
            info.problem = Some(format!("not logged in: run `{}` and log in", agent.program));
        }
        _ => info.available = true,
    }
    info
}

fn login_args(id: &str) -> &'static [&'static str] {
    match id {
        "claude" => &["auth", "status"],
        "codex" => &["login", "status"],
        "cursor" => &["status"],
        "muse" => &["usage"],
        _ => &["--version"],
    }
}

fn parse_login(id: &str, output: &Captured) -> AgentLogin {
    let text = clean(&output.text());
    match id {
        "claude" => parse_claude_auth(&output.stdout),
        "codex" => parse_codex_login(&text),
        "cursor" => parse_cursor_status(&text),
        "muse" => parse_muse_usage(&text),
        _ => AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: None,
        },
    }
}

/// The version from `--version` output: the first word that starts with a digit, else the
/// first line. `2.1.319 (Claude Code)` → `2.1.319`, `codex-cli 0.167.0` → `0.167.0`.
pub fn parse_version(output: &str) -> Option<String> {
    let output = clean(output);
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let word = line
        .split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()));
    Some(word.unwrap_or(line).to_owned())
}

/// `claude auth status` prints JSON: `loggedIn`, `email`, `authMethod`, `subscriptionType`.
pub fn parse_claude_auth(output: &str) -> AgentLogin {
    let Ok(json) = serde_json::from_str::<Value>(output.trim()) else {
        return AgentLogin {
            state: LoginState::Unknown,
            account: None,
            detail: first_line(output),
        };
    };
    let state = match json.get("loggedIn").and_then(Value::as_bool) {
        Some(true) => LoginState::LoggedIn,
        Some(false) => LoginState::LoggedOut,
        None => LoginState::Unknown,
    };
    let text = |key: &str| json.get(key).and_then(Value::as_str).map(str::to_owned);
    let detail = match (text("authMethod"), text("subscriptionType")) {
        (Some(method), Some(plan)) => Some(format!("{method} ({plan})")),
        (Some(method), None) => Some(method),
        (None, plan) => plan,
    };
    AgentLogin {
        state,
        account: text("email"),
        detail,
    }
}

/// `codex login status`: "Logged in using ChatGPT", or "Not logged in".
pub fn parse_codex_login(output: &str) -> AgentLogin {
    let line = first_line(output);
    let lower = output.to_ascii_lowercase();
    let state = if lower.contains("not logged in") {
        LoginState::LoggedOut
    } else if lower.contains("logged in") {
        LoginState::LoggedIn
    } else {
        LoginState::Unknown
    };
    let detail = line
        .as_deref()
        .and_then(|line| line.strip_prefix("Logged in using "))
        .map(str::to_owned)
        .or(line);
    AgentLogin {
        state,
        account: None,
        detail,
    }
}

/// `agent status`: "✓ Logged in as you@example.com", or "Not logged in".
pub fn parse_cursor_status(output: &str) -> AgentLogin {
    let lower = output.to_ascii_lowercase();
    if lower.contains("not logged in") {
        return AgentLogin {
            state: LoginState::LoggedOut,
            account: None,
            detail: first_line(output),
        };
    }
    for line in output.lines() {
        if let Some((_, account)) = line.split_once("Logged in as ") {
            return AgentLogin {
                state: LoginState::LoggedIn,
                account: Some(account.trim().to_owned()),
                detail: None,
            };
        }
    }
    AgentLogin {
        state: if lower.contains("logged in") {
            LoginState::LoggedIn
        } else {
            LoginState::Unknown
        },
        account: None,
        detail: first_line(output),
    }
}

/// `muse usage`: "not logged in", "No subscription allowance (no_allowance)", or the allowance.
pub fn parse_muse_usage(output: &str) -> AgentLogin {
    let detail = first_line(output);
    let lower = output.to_ascii_lowercase();
    let state = if lower.contains("not logged in") || lower.contains("log in") {
        LoginState::LoggedOut
    } else if detail.is_some() {
        LoginState::LoggedIn
    } else {
        LoginState::Unknown
    };
    AgentLogin {
        state,
        account: None,
        detail,
    }
}

/// The first non-empty line, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Removes ANSI escape sequences and zero-width characters, which some CLIs print even when
/// their output isn't a terminal.
fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // CSI: ESC [ … final byte in @..~
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' | '\r' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_as_the_four_clis_print_them() {
        let cases = [
            ("2.1.319 (Claude Code)\n", "2.1.319"),
            ("codex-cli 0.167.0\n", "0.167.0"),
            ("2026.11.05-86047df\r\n", "2026.11.05-86047df"),
            ("Muse Code 1.4.14 (1.4.14-R7309.1)\n", "1.4.14"),
            ("\n  weird build\n", "weird build"),
        ];
        for (output, version) in cases {
            assert_eq!(
                parse_version(output).as_deref(),
                Some(version),
                "{output:?}"
            );
        }
        assert_eq!(parse_version("  \n"), None);
    }

    #[test]
    fn claude_auth_status_json() {
        let login = parse_claude_auth(
            r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "me@example.com", "subscriptionType": "max"}"#,
        );
        assert_eq!(login.state, LoginState::LoggedIn);
        assert_eq!(login.account.as_deref(), Some("me@example.com"));
        assert_eq!(login.detail.as_deref(), Some("claude.ai (max)"));
        assert_eq!(
            parse_claude_auth(r#"{"loggedIn": false}"#).state,
            LoginState::LoggedOut
        );
        let garbled = parse_claude_auth("error: something\n");
        assert_eq!(garbled.state, LoginState::Unknown);
        assert_eq!(garbled.detail.as_deref(), Some("error: something"));
    }

    #[test]
    fn codex_cursor_and_muse_login_lines() {
        let codex = parse_codex_login("Logged in using ChatGPT\n");
        assert_eq!(codex.state, LoginState::LoggedIn);
        assert_eq!(codex.detail.as_deref(), Some("ChatGPT"));
        assert_eq!(
            parse_codex_login("Not logged in\n").state,
            LoginState::LoggedOut
        );

        let cursor =
            parse_cursor_status(&clean("\u{1b}[32m✓\u{1b}[0m Logged in as me@example.com\n"));
        assert_eq!(cursor.state, LoginState::LoggedIn);
        assert_eq!(cursor.account.as_deref(), Some("me@example.com"));
        assert_eq!(
            parse_cursor_status("✗ Not logged in\n").state,
            LoginState::LoggedOut
        );

        let muse = parse_muse_usage("No subscription allowance (no_allowance)\n\n");
        assert_eq!(muse.state, LoginState::LoggedIn);
        assert_eq!(
            muse.detail.as_deref(),
            Some("No subscription allowance (no_allowance)")
        );
        assert_eq!(
            parse_muse_usage("not logged in\n").state,
            LoginState::LoggedOut
        );
        assert_eq!(parse_muse_usage("").state, LoginState::Unknown);
    }
}

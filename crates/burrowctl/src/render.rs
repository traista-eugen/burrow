//! Readable output: session events as a transcript, and one-line summaries of entities.

use std::io::{self, Write};

use burrow_protocol::{
    AgentInfo, ContentBlock, Event, InboxItem, InboxItemStatus, LoginState, PlanEntryStatus,
    Project, ProviderUsage, Session, SessionSummary, StreamEvent, ToolCallContent,
};

/// What the last thing printed was, so streamed text continues on the same line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Inline {
    None,
    Message,
    Thought,
}

/// Prints session events as a transcript.
#[derive(Debug)]
pub struct Transcript {
    inline: Inline,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    /// A transcript that starts on a fresh line.
    pub fn new() -> Self {
        Self {
            inline: Inline::None,
        }
    }

    /// Prints one event.
    pub fn print(&mut self, out: &mut impl Write, event: &StreamEvent) -> io::Result<()> {
        match &event.event {
            Event::AgentMessageChunk(chunk) => self.inline(out, Inline::Message, &chunk.text),
            Event::AgentThoughtChunk(chunk) => self.inline(out, Inline::Thought, &chunk.text),
            other => {
                let line = describe(other);
                self.end_line(out)?;
                if let Some(line) = line {
                    writeln!(out, "{line}")?;
                }
                out.flush()
            }
        }
    }

    /// Ends a line of streamed text, if one is open.
    pub fn end_line(&mut self, out: &mut impl Write) -> io::Result<()> {
        if self.inline != Inline::None {
            writeln!(out)?;
            self.inline = Inline::None;
        }
        Ok(())
    }

    fn inline(&mut self, out: &mut impl Write, kind: Inline, text: &str) -> io::Result<()> {
        if self.inline != kind {
            self.end_line(out)?;
            if kind == Inline::Thought {
                write!(out, "(thinking) ")?;
            }
            self.inline = kind;
        }
        write!(out, "{text}")?;
        out.flush()
    }
}

/// One line for an event that isn't streamed text; `None` to print nothing.
fn describe(event: &Event) -> Option<String> {
    Some(match event {
        Event::UserMessage(message) => {
            let text: Vec<String> = message
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::Image { artifact } => format!("[image {}]", artifact.hash),
                    ContentBlock::Resource { uri } => format!("[resource {uri}]"),
                })
                .collect();
            text.join("\n")
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Event::ToolCall(call) => format!(
            "* {} [{}, {}] ({})",
            call.title, call.kind, call.status, call.tool_call_id
        ),
        Event::ToolCallUpdate(update) => {
            let mut line = format!("  {}:", update.tool_call_id);
            if let Some(status) = &update.status {
                line.push_str(&format!(" {status}"));
            }
            if let Some(title) = &update.title {
                line.push_str(&format!(" \"{title}\""));
            }
            for content in update.content.iter().flatten() {
                match content {
                    ToolCallContent::Content {
                        content: ContentBlock::Text { text },
                    } => line.push_str(&format!(" - {text}")),
                    ToolCallContent::Diff { path, .. } => {
                        line.push_str(&format!(" - diff of {path}"))
                    }
                    ToolCallContent::Terminal { terminal_id } => {
                        line.push_str(&format!(" - terminal {terminal_id}"));
                    }
                    ToolCallContent::Content { .. } => {}
                }
            }
            line
        }
        Event::Plan(plan) => {
            let mut lines = vec!["plan:".to_owned()];
            for entry in &plan.entries {
                let mark = match entry.status {
                    PlanEntryStatus::Completed => "x",
                    PlanEntryStatus::InProgress => "~",
                    _ => " ",
                };
                lines.push(format!("  [{mark}] {}", entry.content));
            }
            lines.join("\n")
        }
        Event::PermissionRequested(request) => {
            let options: Vec<&str> = request
                .options
                .iter()
                .map(|option| option.option_id.as_str())
                .collect();
            format!(
                "? permission needed: {}\n  answer with: burrowctl inbox respond {} <{}>",
                request.title,
                request.inbox_item_id,
                options.join("|")
            )
        }
        Event::PermissionResolved(resolved) => match (&resolved.option_id, &resolved.by) {
            (Some(option), Some(by)) => format!("  permission answered: {option} (by {by})"),
            (Some(option), None) => format!("  permission answered: {option}"),
            _ => "  permission expired".to_owned(),
        },
        Event::TurnEnded(ended) => match &ended.message {
            Some(message) => format!("-- turn ended: {} ({message})", ended.reason),
            None => format!("-- turn ended: {}", ended.reason),
        },
        Event::Usage(usage) => {
            let mut line = format!(
                "  usage: {} tokens in, {} out",
                usage.input_tokens, usage.output_tokens
            );
            if let Some(cost) = usage.cost_usd {
                line.push_str(&format!(", ${cost:.4}"));
            }
            line
        }
        Event::LimitReached(limit) => match limit.resets_at {
            Some(resets_at) => format!(
                "! limit reached ({}): {}; resets at {resets_at}",
                limit.provider, limit.message
            ),
            None => format!("! limit reached ({}): {}", limit.provider, limit.message),
        },
        Event::StateChanged(changed) => match &changed.reason {
            Some(reason) => format!("[{}: {reason}]", changed.state),
            None => format!("[{}]", changed.state),
        },
        Event::ModelChanged(changed) => format!("[model: {}]", changed.model),
        Event::AgentModeChanged(changed) => format!("[agent mode: {}]", changed.agent_mode),
        Event::TerminalOpened(opened) => format!("[terminal {}]", opened.terminal_id),
        Event::AgentMessageChunk(_) | Event::AgentThoughtChunk(_) => return None,
        other => format!("[{}]", other.type_name()),
    })
}

/// `prj_…  name  path  owner/repo`.
pub fn project(project: &Project) -> String {
    let mut line = format!("{}  {}  {}", project.id, project.name, project.path);
    if let Some(remote) = &project.remote {
        match (&remote.owner, &remote.repo) {
            (Some(owner), Some(repo)) => line.push_str(&format!("  {owner}/{repo}")),
            _ => line.push_str(&format!("  {}", remote.url)),
        }
    }
    line
}

/// `claude  Claude Code  2.1.319  logged in (…)  available`.
pub fn agent(agent: &AgentInfo) -> String {
    let version = agent.version.as_deref().unwrap_or("-");
    let login = match agent.login.state {
        LoginState::LoggedIn => "logged in".to_owned(),
        LoginState::LoggedOut => "logged out".to_owned(),
        _ => format!("login {}", agent.login.state),
    };
    let login = match (&agent.login.account, &agent.login.detail) {
        (Some(account), _) => format!("{login} as {account}"),
        (None, Some(detail)) => format!("{login} ({detail})"),
        (None, None) => login,
    };
    let status = if agent.available {
        "available".to_owned()
    } else {
        format!(
            "unavailable: {}",
            agent.problem.as_deref().unwrap_or("unknown reason")
        )
    };
    let mut line = format!(
        "{:<7} {:<12} {:<20} {login}; {status}",
        agent.id.as_str(),
        agent.name,
        version
    );
    // An available agent can still have a problem, such as Muse's sandbox not being set up.
    if agent.available
        && let Some(problem) = &agent.problem
    {
        line.push_str(&format!(
            "
        problem: {problem}"
        ));
    }
    if let Some(adapter) = &agent.adapter {
        let state = if adapter.installed {
            "installed"
        } else {
            "not installed"
        };
        line.push_str(&format!(
            "\n        adapter: {} {} ({state})",
            adapter.package, adapter.version
        ));
    }
    if let Some(mode) = &agent.default_agent_mode {
        line.push_str(&format!("\n        sessions start in mode: {mode}"));
    }
    if !agent.agent_modes.is_empty() {
        let modes: Vec<&str> = agent.agent_modes.iter().map(|m| m.id.as_str()).collect();
        line.push_str(&format!("\n        modes: {}", modes.join(", ")));
    }
    if !agent.models.is_empty() {
        /// Models listed before the rest are counted; `--json` has them all.
        const SHOWN: usize = 8;
        let models: Vec<&str> = agent
            .models
            .iter()
            .take(SHOWN)
            .map(|m| m.id.as_str())
            .collect();
        line.push_str(&format!("\n        models: {}", models.join(", ")));
        if agent.models.len() > SHOWN {
            line.push_str(&format!(" and {} more", agent.models.len() - SHOWN));
        }
    }
    line
}

/// `ses_…  idle  fake  title`.
pub fn session_summary(session: &SessionSummary) -> String {
    let mut line = format!(
        "{}  {:<8} {:<7}",
        session.id,
        session.state.as_str(),
        session.agent.as_str()
    );
    if let Some(title) = &session.title {
        line.push_str(&format!(" {title}"));
    }
    if session.archived {
        line.push_str(" (archived)");
    }
    line
}

/// A few lines about a session.
pub fn session(session: &Session) -> String {
    let mut lines = vec![format!(
        "{}  {}  {}",
        session.id,
        session.state.as_str(),
        session.agent.as_str()
    )];
    if let Some(model) = &session.model {
        lines.push(format!("  model: {model}"));
    }
    if let Some(mode) = &session.agent_mode {
        let others: Vec<&str> = session
            .agent_modes
            .iter()
            .map(|mode| mode.id.as_str())
            .collect();
        if others.is_empty() {
            lines.push(format!("  mode:  {mode}"));
        } else {
            lines.push(format!("  mode:  {mode} (of {})", others.join(", ")));
        }
    }
    lines.push(format!("  cwd:   {}", session.cwd));
    if let Some(reason) = &session.reason {
        lines.push(format!("  why:   {reason}"));
    }
    lines.join("\n")
}

/// `inb_…  open  [permission]  title  (ses_…)  actions: a|b`.
pub fn inbox_item(item: &InboxItem) -> String {
    let status = match item.status {
        InboxItemStatus::Open => "open",
        InboxItemStatus::Resolved => "resolved",
        InboxItemStatus::Expired => "expired",
    };
    let mut line = format!("{}  {:<8} [{}]  {}", item.id, status, item.kind, item.title);
    if let Some(session) = &item.source.session_id {
        line.push_str(&format!("  ({session})"));
    }
    match (&item.status, &item.resolution) {
        (InboxItemStatus::Open, _) => {
            let actions: Vec<&str> = item.actions.iter().map(|a| a.id.as_str()).collect();
            line.push_str(&format!("  actions: {}", actions.join("|")));
        }
        (_, Some(resolution)) => {
            line.push_str(&format!("  -> {} by {}", resolution.action, resolution.by));
        }
        _ => {}
    }
    line
}

/// One provider's usage.
pub fn usage(usage: &ProviderUsage) -> String {
    let mut line = usage.provider.clone();
    if let (Some(used), Some(limit)) = (usage.used, usage.limit) {
        let unit = usage.unit.as_deref().unwrap_or("");
        line.push_str(&format!("  {used}/{limit} {unit}"));
    }
    if usage.limited {
        line.push_str("  LIMITED");
    }
    if let Some(resets_at) = usage.resets_at {
        line.push_str(&format!("  resets at {resets_at}"));
    }
    if let Some(message) = &usage.message {
        line.push_str(&format!("  ({message})"));
    }
    line
}

#[cfg(test)]
mod tests {
    use burrow_protocol::{AgentId, AgentLogin, Integration, Timestamp};

    use super::*;

    fn muse(available: bool, problem: Option<&str>) -> AgentInfo {
        AgentInfo {
            id: AgentId::Muse,
            name: "Muse Code".into(),
            integration: Integration::Msp,
            installed: true,
            version: Some("1.4.14".into()),
            path: None,
            login: AgentLogin {
                state: LoginState::LoggedIn,
                account: None,
                detail: None,
            },
            models: Vec::new(),
            agent_modes: Vec::new(),
            available,
            problem: problem.map(Into::into),
            adapter: None,
            default_agent_mode: Some("promptUnmatched".into()),
            detected_at: Timestamp::now(),
        }
    }

    #[test]
    fn an_available_agent_shows_its_problem_too() {
        let text = agent(&muse(true, Some("Set up the sandbox.")));
        assert!(text.contains("available"), "{text}");
        assert!(text.contains("problem: Set up the sandbox."), "{text}");
        assert!(
            text.contains("sessions start in mode: promptUnmatched"),
            "{text}"
        );

        let text = agent(&muse(false, Some("Not logged in.")));
        assert!(text.contains("unavailable: Not logged in."), "{text}");
        assert!(!text.contains("problem:"), "said twice: {text}");

        let mut with_modes = muse(true, None);
        with_modes.agent_modes = ["promptUnmatched", "allowAll"]
            .into_iter()
            .map(|id| burrow_protocol::AgentMode {
                id: id.into(),
                name: None,
                description: None,
            })
            .collect();
        let text = agent(&with_modes);
        assert!(text.contains("modes: promptUnmatched, allowAll"), "{text}");

        assert!(!agent(&muse(true, None)).contains("problem:"));
    }
}

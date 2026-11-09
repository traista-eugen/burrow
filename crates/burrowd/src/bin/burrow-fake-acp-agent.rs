//! `burrow-fake-acp-agent`: the scripted ACP agent that `burrowd --dev` offers as agent
//! `fake-acp`.
//!
//! It lives in this package so `burrowd`'s integration tests can run it, and so it is built next
//! to `burrowd.exe`, where the ACP adapter looks for it. The script is described in
//! `burrow_agents::acp::fake_agent`.

fn main() -> std::process::ExitCode {
    burrow_agents::acp::fake_agent::main()
}

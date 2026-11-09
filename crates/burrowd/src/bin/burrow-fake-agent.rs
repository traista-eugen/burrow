//! `burrow-fake-agent`: the scripted fake agent that `burrowd --dev` offers as agent `fake`.
//!
//! It lives in this package so `burrowd`'s integration tests can run it, and so it is built next
//! to `burrowd.exe`, where the fake adapter looks for it. The script is described in
//! `burrow_agents::fake::agent`.

fn main() -> std::process::ExitCode {
    burrow_agents::fake::agent::main()
}

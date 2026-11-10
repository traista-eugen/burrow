//! `cargo xtask measure`: the desktop app's startup time and memory, against seeded data.
//!
//! 1. Builds the release `burrow.exe`, `burrowd.exe` and the fake agent (unless
//!    `--skip-build`).
//! 2. Starts a `burrowd --dev --agents fake` of its own, on its own pipe and data folder in
//!    `target/measure/`, and seeds it through `burrow-client`: one project, `--sessions`
//!    sessions with one short turn each, the first `--long` of them with `--turns` turns of a
//!    paragraph of Markdown each. The fake agent spends no quota.
//! 3. Launches `burrow.exe --measure` `--runs` times with `burrowd` already running: the first
//!    frame, when the lists were in, and the memory with every session in the sidebar.
//! 4. Launches it once more with `--open` for each long session: the memory with them open.
//! 5. Stops `burrowd` and launches the app `--runs` times with `--burrowd-args`, so it starts
//!    `burrowd` itself, stopping that one after each run.
//!
//! It prints a table, and the raw JSON of every run. Nothing touches the user's own `burrowd`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use burrow_client::{Client, ConnectError, ConnectOptions};
use burrow_protocol::methods::{ProjectAdd, SessionCreate, SessionGet, SessionPrompt};
use burrow_protocol::{
    AgentId, ClientInfo, ClientKind, ContentBlock, ProjectAddParams, SessionCreateParams,
    SessionIdParams, SessionPromptParams, SessionState,
};
use serde_json::Value;

/// What to measure.
#[derive(Debug)]
pub struct Options {
    runs: usize,
    sessions: usize,
    long: usize,
    turns: usize,
    build: bool,
}

impl Options {
    /// Parses `--runs N --sessions N --long N --turns N --skip-build`.
    pub fn parse(args: &[&str]) -> Result<Self, String> {
        let mut options = Self {
            runs: 5,
            sessions: 50,
            long: 3,
            turns: 100,
            build: true,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut number = |name: &str| -> Result<usize, String> {
                args.next()
                    .and_then(|value| value.parse().ok())
                    .ok_or_else(|| format!("{name} needs a number"))
            };
            match *arg {
                "--runs" => options.runs = number("--runs")?.max(1),
                "--sessions" => options.sessions = number("--sessions")?,
                "--long" => options.long = number("--long")?,
                "--turns" => options.turns = number("--turns")?,
                "--skip-build" => options.build = false,
                other => return Err(format!("measure: unknown argument {other}")),
            }
        }
        options.long = options.long.min(options.sessions);
        Ok(options)
    }
}

/// A process that is killed when dropped.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs the measurement.
pub fn run(root: &Path, options: Options) -> Result<(), String> {
    if options.build {
        println!("building the release binaries…");
        let status = Command::new(env!("CARGO"))
            .current_dir(root)
            .args(["build", "--release", "-p", "burrow-app", "-p", "burrowd"])
            .status()
            .map_err(|error| format!("running cargo: {error}"))?;
        if !status.success() {
            return Err("the release build failed".into());
        }
    }
    let bin = root.join("target").join("release");
    let app = bin.join("burrow.exe");
    let burrowd = bin.join("burrowd.exe");
    for program in [&app, &burrowd, &bin.join("burrow-fake-agent.exe")] {
        if !program.is_file() {
            return Err(format!(
                "{} is missing; run without --skip-build",
                program.display()
            ));
        }
    }

    let work = root
        .join("target")
        .join("measure")
        .join(std::process::id().to_string());
    let data = work.join("data");
    let project = work.join("project");
    fs::create_dir_all(&data).map_err(|error| format!("creating {}: {error}", data.display()))?;
    fs::create_dir_all(&project)
        .map_err(|error| format!("creating {}: {error}", project.display()))?;
    let pipe = format!("burrow-measure-{}", std::process::id());
    let burrowd_args = format!("--dev --agents fake --data-dir {}", data.display());

    let result = (|| {
        let daemon = start_burrowd(&burrowd, &pipe, &burrowd_args)?;
        let runtime =
            burrow_client::io_runtime().map_err(|error| format!("starting tokio: {error}"))?;
        let seeded_at = Instant::now();
        let long = runtime.block_on(seed(&pipe, &project, &options))?;
        println!(
            "seeded {} sessions ({} with {} turns) in {:.1} s",
            options.sessions,
            long.len(),
            options.turns,
            seeded_at.elapsed().as_secs_f64()
        );

        let mut warm = Vec::new();
        for run in 0..options.runs {
            let out = work.join(format!("running-{run}.json"));
            warm.push(launch(&app, &["--pipe", &pipe], &out)?);
        }
        let open: Vec<String> = long
            .iter()
            .flat_map(|id| ["--open".to_owned(), id.clone()])
            .collect();
        let mut args = vec!["--pipe".to_owned(), pipe.clone()];
        args.extend(open);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let opened = launch(&app, &args, &work.join("open.json"))?;
        drop(daemon);

        let mut starting = Vec::new();
        for run in 0..options.runs {
            let out = work.join(format!("starting-{run}.json"));
            let report = launch(
                &app,
                &["--pipe", &pipe, "--burrowd-args", &burrowd_args],
                &out,
            )?;
            if let Some(pid) = report["startedDaemonPid"].as_u64() {
                kill(pid);
            }
            starting.push(report);
        }

        let size = fs::metadata(&app).map(|m| m.len()).unwrap_or(0);
        print_report(&options, size, &warm, &opened, &starting);
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

fn start_burrowd(program: &Path, pipe: &str, args: &str) -> Result<Daemon, String> {
    let child = Command::new(program)
        .args(["--pipe", pipe])
        .args(args.split_whitespace())
        .env("BURROW_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("starting {}: {error}", program.display()))?;
    Ok(Daemon(child))
}

fn options(pipe: &str) -> ConnectOptions {
    ConnectOptions::new(ClientInfo {
        name: "xtask-measure".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        kind: ClientKind::Cli,
    })
    .pipe(pipe.to_owned())
}

/// Seeds `burrowd`; returns the ids of the long sessions.
async fn seed(pipe: &str, project: &Path, options: &Options) -> Result<Vec<String>, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let client = loop {
        match Client::connect(self::options(pipe)).await {
            Ok(client) => break client,
            Err(ConnectError::NotRunning { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(format!("connecting to the measuring burrowd: {error}")),
        }
    };
    let project = client
        .call::<ProjectAdd>(&ProjectAddParams {
            path: project.display().to_string(),
        })
        .await
        .map_err(|error| format!("project/add: {error}"))?;

    let mut tasks = Vec::new();
    for n in 0..options.sessions {
        let client = client.clone();
        let project_id = project.id.clone();
        let turns = if n < options.long { options.turns } else { 1 };
        tasks.push(tokio::spawn(async move {
            let session = client
                .call::<SessionCreate>(&SessionCreateParams {
                    project_id: Some(project_id),
                    agent: AgentId::Fake,
                    model: None,
                    mode: Default::default(),
                    agent_mode: None,
                    cwd: None,
                    title: None,
                    cols: None,
                    rows: None,
                })
                .await
                .map_err(|error| format!("session/create: {error}"))?;
            for turn in 0..turns {
                let text = if turns == 1 {
                    format!("Seed session {n}: say hello")
                } else {
                    long_prompt(n, turn)
                };
                prompt_and_wait(&client, &session.id, text).await?;
            }
            Ok::<_, String>((n, session.id))
        }));
    }
    let mut long = Vec::new();
    for task in tasks {
        let (n, id) = task.await.map_err(|error| error.to_string())??;
        if n < options.long {
            long.push(id);
        }
    }
    Ok(long)
}

async fn prompt_and_wait(client: &Client, session_id: &str, text: String) -> Result<(), String> {
    client
        .call::<SessionPrompt>(&SessionPromptParams {
            session_id: session_id.to_owned(),
            content: vec![ContentBlock::text(text)],
        })
        .await
        .map_err(|error| format!("session/prompt: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let session = client
            .call::<SessionGet>(&SessionIdParams {
                session_id: session_id.to_owned(),
            })
            .await
            .map_err(|error| format!("session/get: {error}"))?;
        match session.state {
            SessionState::Idle => return Ok(()),
            SessionState::Working | SessionState::Waiting => {}
            other => return Err(format!("session {session_id} ended up {other}")),
        }
        if Instant::now() > deadline {
            return Err(format!("session {session_id} didn't finish its turn"));
        }
    }
}

/// A paragraph of Markdown, different every turn, about 80 words.
fn long_prompt(session: usize, turn: usize) -> String {
    format!(
        "## Turn {turn} of session {session}\n\nPlease look at `src/module_{turn}.rs` and explain \
         what **`handle_request`** does when the queue is full. I think it drops the oldest \
         entry, but the comment says it blocks. Also check whether `retry_after` is computed \
         from the server's header or from our own backoff table, and list the call sites:\n\n\
         - the scheduler in `scheduler.rs`\n- the uploader in `upload.rs`\n- anything in the \
         tests that mocks the clock\n\nKeep the answer short, and quote the lines you mean."
    )
}

/// Launches the app with `--measure out` and the given arguments, and reads what it wrote.
fn launch(app: &Path, args: &[&str], out: &PathBuf) -> Result<Value, String> {
    let _ = fs::remove_file(out);
    let mut child = Command::new(app)
        .args(args)
        .arg("--measure")
        .arg(out)
        .env("BURROW_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("starting {}: {error}", app.display()))?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("the app didn't finish measuring within 3 minutes".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let text = fs::read_to_string(out)
        .map_err(|error| format!("the app wrote no measurements ({error})"))?;
    let report: Value =
        serde_json::from_str(&text).map_err(|error| format!("reading {text}: {error}"))?;
    println!("  {}", text.trim());
    if report["timedOut"] == Value::Bool(true) {
        return Err("the app timed out before it was ready".into());
    }
    Ok(report)
}

fn kill(pid: u64) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // Let its pipe close before the next run starts another.
    thread::sleep(Duration::from_millis(300));
}

fn numbers(reports: &[Value], key: &str) -> Vec<f64> {
    reports.iter().filter_map(|r| r[key].as_f64()).collect()
}

fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

fn summary(values: &[f64]) -> String {
    match (
        median(values),
        values.iter().copied().reduce(f64::min),
        values.iter().copied().reduce(f64::max),
    ) {
        (Some(median), Some(min), Some(max)) => format!("{median:.0} (min {min:.0}, max {max:.0})"),
        _ => "-".into(),
    }
}

fn print_report(options: &Options, size: u64, warm: &[Value], opened: &Value, starting: &[Value]) {
    let events = opened["eventsLoaded"].as_u64().unwrap_or(0);
    println!();
    println!(
        "burrow.exe {:.1} MB · {} sessions, {} open with {} events · median of {} runs",
        size as f64 / 1_048_576.0,
        options.sessions,
        options.long,
        events,
        options.runs
    );
    println!();
    println!("| Measure | burrowd running | app starts burrowd |");
    println!("|---|---|---|");
    for (label, key) in [
        ("First frame (ms after process start)", "firstFrameMs"),
        ("Connected (ms)", "connectedMs"),
        ("Lists and agents loaded (ms)", "readyMs"),
        ("Working set (MB)", "workingSetMb"),
        ("Private bytes (MB)", "privateMb"),
    ] {
        println!(
            "| {label} | {} | {} |",
            summary(&numbers(warm, key)),
            summary(&numbers(starting, key))
        );
    }
    println!();
    println!(
        "With {} sessions open ({events} events): working set {} MB, private {} MB, peak {} MB.",
        options.long, opened["workingSetMb"], opened["privateMb"], opened["peakWorkingSetMb"]
    );
}

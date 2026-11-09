//! Command-line arguments and environment.
//!
//! | Argument | Environment | Meaning |
//! |---|---|---|
//! | `--pipe <name>` | `BURROW_PIPE` | Connect to another pipe, for example a test `burrowd`'s |
//! | `--burrowd-args "<args>"` | `BURROWD_ARGS` | Arguments for the `burrowd` the app starts |
//! | `--open <sessionId>` | | Open this session at startup; repeat for several (the last shows) |
//! | `--measure <file>` | | Measure startup and memory, write them to `<file>` as JSON, and exit (see [`crate::startup`]) |
//!
//! On the default pipe the app starts `burrowd` when it isn't running. On another pipe it starts
//! one only when given `--burrowd-args` (it adds `--pipe`): a second `burrowd` on the default
//! data folder would mark the first one's sessions stopped (D-029).

use std::ffi::OsString;

use crate::backend::{BackendConfig, DaemonLaunch, daemon};

/// What the app was started with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    /// The pipe to connect to; `None` for the default.
    pub pipe: Option<String>,
    /// Extra arguments for a `burrowd` the app starts, split on whitespace.
    pub burrowd_args: Option<Vec<String>>,
    /// Sessions to open at startup, in order.
    pub open: Vec<String>,
    /// Where to write startup and memory measurements, then exit.
    pub measure: Option<std::path::PathBuf>,
    /// Arguments that weren't understood.
    pub unknown: Vec<String>,
}

impl Args {
    /// Parses the process's arguments and environment.
    pub fn from_env() -> Self {
        Self::parse(
            std::env::args_os().skip(1),
            std::env::var("BURROW_PIPE").ok(),
            std::env::var("BURROWD_ARGS").ok(),
        )
    }

    /// Parses `args`, falling back to the environment values given.
    pub fn parse(
        args: impl IntoIterator<Item = OsString>,
        env_pipe: Option<String>,
        env_burrowd_args: Option<String>,
    ) -> Self {
        let mut parsed = Args::default();
        let mut args = args
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned());
        while let Some(arg) = args.next() {
            if let Some(value) = arg.strip_prefix("--pipe=") {
                parsed.pipe = Some(value.to_owned());
            } else if arg == "--pipe" {
                parsed.pipe = args.next();
            } else if let Some(value) = arg.strip_prefix("--burrowd-args=") {
                parsed.burrowd_args = Some(split(value));
            } else if arg == "--burrowd-args" {
                parsed.burrowd_args = args.next().map(|value| split(&value));
            } else if let Some(value) = arg.strip_prefix("--open=") {
                parsed.open.push(value.to_owned());
            } else if arg == "--open" {
                parsed.open.extend(args.next());
            } else if let Some(value) = arg.strip_prefix("--measure=") {
                parsed.measure = Some(value.into());
            } else if arg == "--measure" {
                parsed.measure = args.next().map(Into::into);
            } else {
                parsed.unknown.push(arg);
            }
        }
        if parsed.pipe.is_none() {
            parsed.pipe = env_pipe.filter(|pipe| !pipe.trim().is_empty());
        }
        if parsed.burrowd_args.is_none() {
            parsed.burrowd_args = env_burrowd_args.map(|value| split(&value));
        }
        parsed
    }

    /// Where to connect, and how to start `burrowd` there, if the app may.
    pub fn backend_config(&self) -> BackendConfig {
        let program = daemon::next_to_current_exe().ok();
        let daemon = match (&self.pipe, &self.burrowd_args, program) {
            (_, _, None) => None,
            (None, extra, Some(program)) => Some(DaemonLaunch {
                program,
                args: extra.clone().unwrap_or_default(),
            }),
            (Some(pipe), Some(extra), Some(program)) => {
                let mut args = vec!["--pipe".to_owned(), pipe.clone()];
                args.extend(extra.iter().cloned());
                Some(DaemonLaunch { program, args })
            }
            (Some(_), None, Some(_)) => None,
        };
        BackendConfig {
            pipe: self.pipe.clone(),
            daemon,
        }
    }
}

fn split(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], pipe: Option<&str>, extra: Option<&str>) -> Args {
        Args::parse(
            args.iter().map(OsString::from),
            pipe.map(Into::into),
            extra.map(Into::into),
        )
    }

    #[test]
    fn flags_beat_the_environment() {
        let args = parse(&["--pipe", "burrow-test"], Some("from-env"), None);
        assert_eq!(args.pipe.as_deref(), Some("burrow-test"));
        let args = parse(
            &["--pipe=x", "--burrowd-args=--dev --data-dir D:\\t"],
            None,
            Some("--other"),
        );
        assert_eq!(args.pipe.as_deref(), Some("x"));
        assert_eq!(args.burrowd_args.unwrap(), ["--dev", "--data-dir", "D:\\t"]);
        let args = parse(&["--what"], Some("from-env"), Some("--dev"));
        assert_eq!(args.pipe.as_deref(), Some("from-env"));
        assert_eq!(args.burrowd_args.unwrap(), ["--dev"]);
        assert_eq!(args.unknown, ["--what"]);
        assert_eq!(parse(&[], Some("  "), None).pipe, None);
    }

    #[test]
    fn sessions_to_open_and_measuring() {
        let args = parse(
            &["--open", "ses_1", "--open=ses_2", "--measure", r"D:\m.json"],
            None,
            None,
        );
        assert_eq!(args.open, ["ses_1", "ses_2"]);
        assert_eq!(
            args.measure.as_deref(),
            Some(std::path::Path::new(r"D:\m.json"))
        );
        assert!(args.unknown.is_empty());
    }

    #[test]
    fn burrowd_is_started_on_the_default_pipe_or_when_asked() {
        let config = parse(&[], None, None).backend_config();
        assert_eq!(config.pipe, None);
        let launch = config.daemon.unwrap();
        assert!(launch.program.ends_with("burrowd.exe") || launch.program.ends_with("burrowd"));
        assert!(launch.args.is_empty());

        assert_eq!(
            parse(&["--pipe", "t"], None, None).backend_config().daemon,
            None
        );

        let launch = parse(&["--pipe", "t"], None, Some("--dev"))
            .backend_config()
            .daemon
            .unwrap();
        assert_eq!(launch.args, ["--pipe", "t", "--dev"]);
    }
}

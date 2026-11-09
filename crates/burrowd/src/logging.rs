//! Logging: `tracing`, written to `<data dir>\logs\burrowd.<date>.log` with daily rotation, and
//! to stderr when `burrowd` runs in a console.
//!
//! The level comes from the `BURROW_LOG` environment variable, in `tracing_subscriber`'s
//! `EnvFilter` syntax (for example `debug`, or `info,burrowd=trace`). The default is `info`.
//! Lines are written synchronously, so the last ones before a crash aren't lost.

use std::error::Error;
use std::io::IsTerminal;
use std::path::Path;

use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// The environment variable that sets the log filter.
pub const LOG_ENV: &str = "BURROW_LOG";
/// The filter used when `BURROW_LOG` is unset or invalid.
const DEFAULT_FILTER: &str = "info";
/// Daily log files kept; older ones are deleted.
const MAX_LOG_FILES: usize = 14;

/// Starts logging to files in `dir`, creating it if needed, and to stderr if it's a console.
/// Panics are logged too.
pub fn init(dir: &Path) -> Result<(), Box<dyn Error + Send + Sync>> {
    std::fs::create_dir_all(dir)?;
    let file = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("burrowd")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(dir)?;

    let filter =
        EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let stderr = std::io::stderr()
        .is_terminal()
        .then(|| fmt::layer().with_writer(std::io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(file).with_ansi(false))
        .with(stderr)
        .try_init()?;

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(%info, "panic");
        default_hook(info);
    }));
    Ok(())
}

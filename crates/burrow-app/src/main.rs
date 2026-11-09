//! `burrow.exe`, the desktop client of `burrowd`.
//!
//! Module map:
//! - [`backend`]: the connection to `burrowd` (reconnects, starting it, resumable streams);
//! - [`state`]: shared state as GPUI entities, with plain models underneath;
//! - [`views`]: the window, laid out as described there;
//! - [`actions`], [`theme`], [`assets`], [`args`], [`startup`]: keys, looks, icons, arguments,
//!   startup and memory measurements.

// Release builds are GUI apps: no console window behind the main window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod args;
mod assets;
mod backend;
mod startup;
mod state;
mod theme;
mod views;

use std::time::Instant;

use gpui::{
    App, AppContext as _, Bounds, Size, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use gpui_component::Root;
use tracing::warn;

use crate::args::Args;
use crate::backend::Backend;
use crate::state::AppState;
use crate::views::workspace::Workspace;

fn main() {
    let main_started = Instant::now();
    init_logging();
    let args = Args::from_env();
    if !args.unknown.is_empty() {
        warn!(unknown = ?args.unknown, "ignoring arguments");
    }
    let backend = match Backend::new(args.backend_config()) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("burrow: can't start the I/O runtime: {error}");
            std::process::exit(1);
        }
    };

    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            theme::apply(cx);
            actions::bind_keys(cx);
            cx.set_global(backend);
            let state = cx.new(AppState::new);

            // Quit with the last window. burrowd and its agents keep running.
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let first_frame = startup::FirstFrameCell::default();
            let open = args.open.clone();
            let measure = args.measure.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(Size::new(px(760.), px(480.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Burrow".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let cell = first_frame.clone();
                    window.on_next_frame(move |_, _| {
                        cell.set(Some(startup::report_first_frame(main_started)));
                    });
                    let workspace = cx.new(|cx| Workspace::new(state.clone(), window, cx));
                    for session_id in open {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_session(session_id, window, cx)
                        });
                    }
                    if let Some(out) = measure {
                        startup::measure(out, first_frame, state, workspace.clone(), cx);
                    }
                    cx.new(|cx| Root::new(workspace, window, cx))
                },
            );
            if let Err(error) = opened {
                eprintln!("burrow: can't open the main window: {error}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}

/// Logs to stderr. `BURROW_LOG` sets the filter, for example `debug`; the default is `info`.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("BURROW_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .try_init();
}

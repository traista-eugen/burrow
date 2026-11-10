//! One attached client's `terminal/update`s and scrollback pages (D-039).
//!
//! The task wakes when the terminal's screen changes, waits until [`FRAME`] has passed since
//! its last update, then compares the screen with what it last sent ([`super::Screen::diff`])
//! and sends what differs. It sends the next update only once the previous one has been written
//! to the client's pipe. So a burst of output costs a client one update per frame, and a client
//! that reads slowly gets the latest screen when it catches up, never a backlog; the terminal's
//! reader never waits for it.
//!
//! The same task answers the client's `terminal/scrollback` requests, so each answer's indexes
//! are relative to the screen as the client has it: the updates before the answer are exactly
//! the ones it has seen.

use std::sync::Arc;
use std::time::Duration;

use burrow_protocol::methods::TerminalScrollback;
use burrow_protocol::notifications::{TerminalClosedNotification, TerminalUpdated};
use burrow_protocol::{
    Notification, RequestId, Response, TerminalClosedParams, TerminalScrollbackParams,
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use super::{Frame, Terminal};
use crate::connection::Outgoing;

/// The shortest time between two updates to one client: about one display frame.
pub const FRAME: Duration = Duration::from_millis(16);

/// A `terminal/scrollback` request from the attached client.
pub type PageRequest = (RequestId, TerminalScrollbackParams);

/// Sends one client the updates of one terminal, and answers its scrollback requests.
pub struct Updates {
    pub terminal: Arc<Terminal>,
    /// What the client has: the snapshot's frame, then each update's.
    pub sent: Frame,
    pub changed: watch::Receiver<u64>,
    pub requests: mpsc::UnboundedReceiver<PageRequest>,
    pub out: mpsc::Sender<Outgoing>,
}

impl Updates {
    /// Runs until the terminal closes (after sending `terminal/closed`), or the connection
    /// does. Detaching aborts it.
    pub async fn run(mut self) {
        let mut last = Instant::now();
        loop {
            tokio::select! {
                changed = self.changed.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    tokio::time::sleep_until(last + FRAME).await;
                    last = Instant::now();
                    match self.update().await {
                        Ok(true) => {}
                        Ok(false) | Err(()) => break,
                    }
                }
                Some(request) = self.requests.recv() => {
                    if self.answer(request).await.is_err() {
                        return;
                    }
                }
            }
        }
        // Requests that came meanwhile still get their answers; later ones go elsewhere.
        self.requests.close();
        while let Some(request) = self.requests.recv().await {
            if self.answer(request).await.is_err() {
                return;
            }
        }
    }

    /// Sends what changed, if anything, and waits until it is written. `Ok(false)` once the
    /// terminal has closed and `terminal/closed` is sent; `Err` when the connection is gone.
    async fn update(&mut self) -> Result<bool, ()> {
        // Changes until now are in this update.
        self.changed.borrow_and_update();
        let (frame, update, exit) = self.terminal.update_for(&self.sent);
        if let Some(update) = update {
            let line = Notification::typed::<TerminalUpdated>(&update).to_line();
            let (written, done) = oneshot::channel();
            self.out
                .send(Outgoing::tracked(line.into(), written))
                .await
                .map_err(drop)?;
            // Until it is in the pipe; meanwhile the screen may change many times.
            done.await.map_err(drop)?;
        }
        self.sent = frame;
        let Some(exit) = exit else {
            return Ok(true);
        };
        let closed = TerminalClosedParams {
            terminal_id: self.terminal.id().to_owned(),
            exit_code: exit.exit_code,
        };
        let line = Notification::typed::<TerminalClosedNotification>(&closed).to_line();
        self.out.send(Outgoing::from(line)).await.map_err(drop)?;
        Ok(false)
    }

    async fn answer(&mut self, (id, params): PageRequest) -> Result<(), ()> {
        let result = self
            .terminal
            .page(Some(&self.sent), params.before, params.limit);
        let line = Response::ok::<TerminalScrollback>(id, &result).to_line();
        self.out.send(Outgoing::from(line)).await.map_err(drop)
    }
}

//! Terminals the app shows: attachments to `burrowd`'s terminals, kept across reconnects.
//!
//! - [`TerminalHub`] (one per app, owned by [`super::AppState`]) is the app's one reader of the
//!   connection's notifications. Views register a terminal with [`TerminalHub::attach`] and a
//!   sink; the hub attaches, routes `terminal/update` and `terminal/closed` to the sink, and
//!   attaches again with a fresh snapshot after every reconnect.
//! - [`Router`] is its plain model (no GPUI, D-030): which registration each message belongs to.
//! - [`Sizing`] is the size rule a view follows (D-040), also plain.
//!
//! # Order
//!
//! A terminal's updates are diffs against what this connection was sent before, so they must be
//! applied in the order `burrowd` sent them, after the snapshot they follow, and a scrollback
//! page after exactly the updates sent before it. `burrow-client` answers calls on their own
//! channel, which can be seen before or after the notifications around them, so the hub sends
//! `terminal/attach`, `terminal/detach` and `terminal/scrollback` with
//! [`Client::send_in_order`]: their answers come through the notifications, in order.
//!
//! Each registration has a key. An update is delivered only to the registration whose attach
//! answer came before it, so an update of an earlier attachment, still on its way after a
//! detach, never reaches a later one. Input and resizes go as plain calls; one connection
//! writes requests in the order they are made, and `burrowd` handles a connection's terminal
//! requests in that order.

use std::collections::HashMap;
use std::rc::Rc;

use burrow_client::{CallError, Client, Incoming};
use burrow_protocol::methods::{
    TerminalAttach, TerminalDetach, TerminalInput, TerminalResize, TerminalScrollback,
};
use burrow_protocol::notifications::{TerminalClosedNotification, TerminalUpdated};
use burrow_protocol::{
    Empty, NotificationMethod, RequestId, TerminalIdParams, TerminalInputParams, TerminalLine,
    TerminalResizeParams, TerminalScrollbackParams, TerminalSize, TerminalSnapshot, TerminalUpdate,
};
use futures::FutureExt as _;
use gpui::{App, AppContext as _, AsyncApp, Context, Task, WeakEntity};
use tracing::{debug, warn};

use crate::backend::Backend;

/// The most lines one `terminal/scrollback` returns.
pub const MAX_PAGE: u32 = 1000;
/// The most incoming messages handled in one update of the hub.
const MAX_BATCH: usize = 256;

/// What a registered terminal's sink is told, in the order it happened.
#[derive(Clone, Debug, PartialEq)]
pub enum TerminalMessage {
    /// Attached, or attached again after a reconnect: the whole screen. Updates follow.
    Snapshot(TerminalSnapshot),
    /// What changed since the previous snapshot or update.
    Update(TerminalUpdate),
    /// A page of scrollback the view asked for: lines at negative indexes, oldest first.
    /// `complete`: nothing older is kept.
    History {
        lines: Vec<TerminalLine>,
        complete: bool,
    },
    /// The program exited; no updates follow.
    Closed { exit_code: Option<i32> },
    /// The connection to `burrowd` dropped. A snapshot follows once it is back.
    Disconnected,
    /// Attaching failed, for example because the terminal is gone after `burrowd` restarted.
    Failed(String),
}

/// Identifies one registration of a terminal.
pub type Key = u64;

/// A request the router asks to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outgoing {
    Attach(String),
    Detach(String),
    Scrollback {
        terminal: String,
        before: i64,
        limit: u32,
    },
}

/// Where the router's requests go: the connection, or a test's list.
pub trait Wire {
    /// Sends `request` in order ([`Client::send_in_order`]). `None` when the connection is gone.
    fn send(&mut self, request: &Outgoing) -> Option<RequestId>;
}

impl Wire for Client {
    fn send(&mut self, request: &Outgoing) -> Option<RequestId> {
        let sent = match request {
            Outgoing::Attach(terminal) => self.send_in_order::<TerminalAttach>(&TerminalIdParams {
                terminal_id: terminal.clone(),
            }),
            Outgoing::Detach(terminal) => self.send_in_order::<TerminalDetach>(&TerminalIdParams {
                terminal_id: terminal.clone(),
            }),
            Outgoing::Scrollback {
                terminal,
                before,
                limit,
            } => self.send_in_order::<TerminalScrollback>(&TerminalScrollbackParams {
                terminal_id: terminal.clone(),
                before: Some(*before),
                limit: Some(*limit),
            }),
        };
        sent.ok()
    }
}

/// Where a registration's attachment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Not connected: it attaches once the connection is up.
    Waiting,
    /// `terminal/attach` is on its way.
    Attaching,
    /// Attached: updates are delivered.
    Attached,
    /// Attaching failed; tried again after a reconnect.
    Failed,
}

#[derive(Debug)]
struct Entry {
    key: Key,
    phase: Phase,
}

/// What an answer is for.
#[derive(Debug)]
enum Asked {
    Attach { terminal: String, key: Key },
    Detach,
    Scrollback { terminal: String, key: Key },
}

/// Which registration each in-order message belongs to. One registration per terminal: a new
/// one for a terminal already registered replaces it, detaching and attaching again so it gets
/// its own snapshot.
#[derive(Debug, Default)]
pub struct Router {
    entries: HashMap<String, Entry>,
    asked: HashMap<RequestId, Asked>,
    connected: bool,
    next_key: Key,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `terminal`, attaching now if connected. Returns the registration's key.
    pub fn register(&mut self, terminal: &str, wire: &mut impl Wire) -> Key {
        self.next_key += 1;
        let key = self.next_key;
        let attached = self
            .entries
            .get(terminal)
            .is_some_and(|entry| matches!(entry.phase, Phase::Attaching | Phase::Attached));
        let mut entry = Entry {
            key,
            phase: Phase::Waiting,
        };
        if self.connected {
            if attached {
                self.send(wire, Outgoing::Detach(terminal.to_owned()), Asked::Detach);
            }
            entry.phase = self.attach(terminal, key, wire);
        }
        self.entries.insert(terminal.to_owned(), entry);
        key
    }

    /// Ends registration `key` of `terminal`, detaching it. A key that was replaced does nothing.
    pub fn unregister(&mut self, terminal: &str, key: Key, wire: &mut impl Wire) {
        let Some(entry) = self.entries.get(terminal) else {
            return;
        };
        if entry.key != key {
            return;
        }
        let attached = matches!(entry.phase, Phase::Attaching | Phase::Attached);
        self.entries.remove(terminal);
        if attached && self.connected {
            self.send(wire, Outgoing::Detach(terminal.to_owned()), Asked::Detach);
        }
    }

    /// Whether registration `key` of `terminal` is attached.
    pub fn is_attached(&self, terminal: &str, key: Key) -> bool {
        self.entries
            .get(terminal)
            .is_some_and(|entry| entry.key == key && entry.phase == Phase::Attached)
    }

    /// Asks for up to `limit` scrollback lines above `before`, answered as
    /// [`TerminalMessage::History`]. Returns whether it was sent: only while attached.
    pub fn scrollback(
        &mut self,
        terminal: &str,
        key: Key,
        before: i64,
        limit: u32,
        wire: &mut impl Wire,
    ) -> bool {
        if !self.is_attached(terminal, key) {
            return false;
        }
        let request = Outgoing::Scrollback {
            terminal: terminal.to_owned(),
            before: before.min(0),
            limit: limit.clamp(1, MAX_PAGE),
        };
        let asked = Asked::Scrollback {
            terminal: terminal.to_owned(),
            key,
        };
        self.send(wire, request, asked)
    }

    /// The connection is up: attach every registered terminal.
    pub fn connected(&mut self, wire: &mut impl Wire) {
        self.connected = true;
        self.asked.clear();
        let registered: Vec<(String, Key)> = self
            .entries
            .iter()
            .map(|(terminal, entry)| (terminal.clone(), entry.key))
            .collect();
        for (terminal, key) in registered {
            let phase = self.attach(&terminal, key, wire);
            if let Some(entry) = self.entries.get_mut(&terminal) {
                entry.phase = phase;
            }
        }
    }

    /// The connection dropped: every registration waits for the next one.
    pub fn disconnected(&mut self) -> Vec<(Key, TerminalMessage)> {
        self.connected = false;
        self.asked.clear();
        self.entries
            .values_mut()
            .map(|entry| {
                entry.phase = Phase::Waiting;
                (entry.key, TerminalMessage::Disconnected)
            })
            .collect()
    }

    /// Handles one incoming message: what to tell which registration.
    pub fn incoming(&mut self, incoming: Incoming) -> Option<(Key, TerminalMessage)> {
        match incoming {
            Incoming::Notification(notification) => match notification.method.as_str() {
                TerminalUpdated::NAME => match notification.params_as::<TerminalUpdated>() {
                    Ok(update) => {
                        let key = self.attached_key(&update.terminal_id)?;
                        Some((key, TerminalMessage::Update(update)))
                    }
                    Err(error) => {
                        warn!(%error, "can't decode a terminal/update");
                        None
                    }
                },
                TerminalClosedNotification::NAME => {
                    match notification.params_as::<TerminalClosedNotification>() {
                        Ok(closed) => {
                            let key = self.attached_key(&closed.terminal_id)?;
                            let exit_code = closed.exit_code;
                            Some((key, TerminalMessage::Closed { exit_code }))
                        }
                        Err(error) => {
                            warn!(%error, "can't decode a terminal/closed");
                            None
                        }
                    }
                }
                _ => None,
            },
            Incoming::Answer(response) => {
                let id = response.id.clone()?;
                match self.asked.remove(&id)? {
                    Asked::Detach => None,
                    Asked::Attach { terminal, key } => {
                        let entry = self.entries.get_mut(&terminal)?;
                        if entry.key != key || entry.phase != Phase::Attaching {
                            // A registration that was replaced or ended meanwhile; a detach
                            // already followed this attach.
                            return None;
                        }
                        match response.result_as::<TerminalAttach>() {
                            Ok(snapshot) => {
                                entry.phase = Phase::Attached;
                                Some((key, TerminalMessage::Snapshot(snapshot)))
                            }
                            Err(error) => {
                                entry.phase = Phase::Failed;
                                Some((key, TerminalMessage::Failed(error.message)))
                            }
                        }
                    }
                    Asked::Scrollback { terminal, key } => {
                        self.attached_key(&terminal).filter(|k| *k == key)?;
                        let message = match response.result_as::<TerminalScrollback>() {
                            Ok(page) => TerminalMessage::History {
                                lines: page.lines,
                                complete: page.complete,
                            },
                            Err(error) => {
                                // Paging again would fail the same way.
                                warn!(terminal, error = error.message, "couldn't page back");
                                TerminalMessage::History {
                                    lines: Vec::new(),
                                    complete: true,
                                }
                            }
                        };
                        Some((key, message))
                    }
                }
            }
        }
    }

    fn attached_key(&self, terminal: &str) -> Option<Key> {
        self.entries
            .get(terminal)
            .filter(|entry| entry.phase == Phase::Attached)
            .map(|entry| entry.key)
    }

    fn attach(&mut self, terminal: &str, key: Key, wire: &mut impl Wire) -> Phase {
        let asked = Asked::Attach {
            terminal: terminal.to_owned(),
            key,
        };
        if self.send(wire, Outgoing::Attach(terminal.to_owned()), asked) {
            Phase::Attaching
        } else {
            Phase::Waiting
        }
    }

    fn send(&mut self, wire: &mut impl Wire, request: Outgoing, asked: Asked) -> bool {
        match wire.send(&request) {
            Some(id) => {
                self.asked.insert(id, asked);
                true
            }
            None => false,
        }
    }
}

/// Which size this client asks a terminal for, under "the latest wins" (D-040): a client that
/// only looks changes nothing; one that types, or started the session, sets the size.
///
/// - The view reports the grid that fits it ([`Sizing::view_fits`]). While this client holds the
///   size, it follows at once. Otherwise it waits until the user types
///   ([`Sizing::before_input`]), so a glance from here doesn't reflow another client's screen.
/// - Asking for the size the terminal already has changes nothing, and lets `burrowd` remember
///   it for this client, so it is asked for at once.
/// - Attaching (again, after a reconnect) makes `burrowd` forget what this client asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sizing {
    /// The grid that fits the view, once it has laid out.
    wanted: Option<TerminalSize>,
    /// What this client last asked for on its current attachment.
    asked: Option<TerminalSize>,
    /// The terminal's size, from the latest snapshot or reset.
    current: Option<TerminalSize>,
    /// This client set the size last: it follows the view's size at once.
    holds: bool,
}

impl Sizing {
    /// `holds`: this client started the terminal at its size.
    pub fn new(holds: bool) -> Self {
        Self {
            holds,
            ..Self::default()
        }
    }

    /// Whether this client set the terminal's size last.
    pub fn holds(&self) -> bool {
        self.holds
    }

    /// Attached, at `size`. Returns a size to ask for now.
    pub fn attached(&mut self, size: TerminalSize) -> Option<TerminalSize> {
        let before = self.current.replace(size);
        let asked = self.asked.take();
        // Attached again at another size than it had: another client resized it meanwhile.
        if before.is_some_and(|before| before != size) && asked != Some(size) {
            self.holds = false;
        }
        match self.wanted {
            Some(wanted) if self.holds || wanted == size => self.ask(wanted),
            _ => None,
        }
    }

    /// The terminal is `size` now (a `reset`). Someone else's size ends this client's hold.
    pub fn resized(&mut self, size: TerminalSize) {
        self.current = Some(size);
        if self.asked != Some(size) {
            self.holds = false;
        }
    }

    /// The view fits `size` now. Returns a size to ask for now.
    pub fn view_fits(&mut self, size: TerminalSize) -> Option<TerminalSize> {
        self.wanted = Some(size);
        // Not attached yet: asked for once the snapshot says the terminal's size.
        self.current?;
        if self.holds || self.current == Some(size) {
            return self.ask(size);
        }
        None
    }

    /// The user is about to type. Returns a size to ask for first. From now on this client
    /// holds the size: `burrowd` gives a terminal the size of whoever types.
    pub fn before_input(&mut self) -> Option<TerminalSize> {
        let wanted = self.wanted?;
        self.holds = true;
        self.ask(wanted)
    }

    fn ask(&mut self, size: TerminalSize) -> Option<TerminalSize> {
        if self.asked == Some(size) {
            return None;
        }
        self.asked = Some(size);
        Some(size)
    }
}

/// Where a registration's messages go.
pub type Sink = Rc<dyn Fn(TerminalMessage, &mut App)>;

/// The app's attachments to terminals, across reconnects. See the module docs.
pub struct TerminalHub {
    router: Router,
    client: Option<Client>,
    sinks: HashMap<Key, Sink>,
    _reader: Task<()>,
}

/// A connection that is gone: sends nothing.
struct Nowhere;

impl Wire for Nowhere {
    fn send(&mut self, _: &Outgoing) -> Option<RequestId> {
        None
    }
}

impl TerminalHub {
    /// Starts reading each connection's notifications. Needs the [`Backend`] global.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let reader = cx.spawn(async move |this, cx| read(this, cx).await);
        Self {
            router: Router::new(),
            client: None,
            sinks: HashMap::new(),
            _reader: reader,
        }
    }

    /// Attaches `terminal`, now or once connected, and again after every reconnect. `sink` gets
    /// its messages in order, starting with a [`TerminalMessage::Snapshot`]. Returns the key
    /// to detach it with.
    pub fn attach(
        &mut self,
        terminal: &str,
        sink: impl Fn(TerminalMessage, &mut App) + 'static,
    ) -> Key {
        let key = match self.client.clone() {
            Some(mut client) => self.router.register(terminal, &mut client),
            None => self.router.register(terminal, &mut Nowhere),
        };
        self.sinks.insert(key, Rc::new(sink));
        key
    }

    /// Detaches registration `key` of `terminal`.
    pub fn detach(&mut self, terminal: &str, key: Key) {
        self.sinks.remove(&key);
        match self.client.clone() {
            Some(mut client) => self.router.unregister(terminal, key, &mut client),
            None => self.router.unregister(terminal, key, &mut Nowhere),
        }
    }

    /// Whether registration `key` of `terminal` is attached.
    #[cfg(test)]
    pub fn is_attached(&self, terminal: &str, key: Key) -> bool {
        self.router.is_attached(terminal, key)
    }

    /// Asks for scrollback above `before`; the page arrives as [`TerminalMessage::History`].
    /// Returns whether it was asked: only while attached.
    pub fn scrollback(&mut self, terminal: &str, key: Key, before: i64, limit: u32) -> bool {
        match self.client.clone() {
            Some(mut client) => self
                .router
                .scrollback(terminal, key, before, limit, &mut client),
            None => false,
        }
    }

    /// Sends what the user typed (`terminal/input`).
    pub fn input(&self, terminal: &str, data: String, cx: &App) {
        let params = TerminalInputParams {
            terminal_id: terminal.to_owned(),
            data,
        };
        self.call::<TerminalInput>(params, cx);
    }

    /// Asks for a size (`terminal/resize`).
    pub fn resize(&self, terminal: &str, size: TerminalSize, cx: &App) {
        let params = TerminalResizeParams {
            terminal_id: terminal.to_owned(),
            cols: size.cols,
            rows: size.rows,
        };
        self.call::<TerminalResize>(params, cx);
    }

    /// Sends a call now, so calls go out in the order they are made, and logs its failure.
    fn call<M>(&self, params: M::Params, cx: &App)
    where
        M: burrow_protocol::Method<Result = Empty> + 'static,
    {
        let Some(client) = &self.client else {
            return;
        };
        match client.start_call::<M>(&params) {
            Ok(call) => cx
                .background_spawn(async move {
                    match call.result().await {
                        Ok(_) | Err(CallError::Disconnected) => {}
                        // Typing into a program that just exited, say.
                        Err(error) => debug!(method = M::NAME, %error, "a terminal call failed"),
                    }
                })
                .detach(),
            Err(error) => debug!(method = M::NAME, %error, "a terminal call wasn't sent"),
        }
    }

    fn on_connected(&mut self, client: Client) {
        self.client = Some(client.clone());
        let mut wire = client;
        self.router.connected(&mut wire);
    }

    fn on_disconnected(&mut self) -> Vec<(Sink, TerminalMessage)> {
        self.client = None;
        let messages = self.router.disconnected();
        self.sinks_for(messages)
    }

    fn on_incoming(&mut self, batch: Vec<Incoming>) -> Vec<(Sink, TerminalMessage)> {
        let messages: Vec<(Key, TerminalMessage)> = batch
            .into_iter()
            .filter_map(|incoming| self.router.incoming(incoming))
            .collect();
        self.sinks_for(messages)
    }

    fn sinks_for(&self, messages: Vec<(Key, TerminalMessage)>) -> Vec<(Sink, TerminalMessage)> {
        messages
            .into_iter()
            .filter_map(|(key, message)| Some((self.sinks.get(&key)?.clone(), message)))
            .collect()
    }
}

/// Reads each connection's notifications for as long as the hub lives, and hands each sink its
/// messages outside the hub's update, so a sink can call the hub.
async fn read(this: WeakEntity<TerminalHub>, cx: &mut AsyncApp) {
    let backend = cx.update(|cx| Backend::global(cx).clone());
    loop {
        let client = backend.connected().await;
        // Listen first: notifications that arrive while nobody listens are dropped.
        let mut notifications = client.notifications();
        if this
            .update(cx, |hub, _| hub.on_connected(client.clone()))
            .is_err()
        {
            return;
        }
        while let Some(first) = notifications.next_incoming().await {
            let mut batch = vec![first];
            while batch.len() < MAX_BATCH {
                match notifications.next_incoming().now_or_never() {
                    Some(Some(incoming)) => batch.push(incoming),
                    _ => break,
                }
            }
            let Ok(deliveries) = this.update(cx, |hub, _| hub.on_incoming(batch)) else {
                return;
            };
            deliver(deliveries, cx);
        }
        let Ok(deliveries) = this.update(cx, |hub, _| hub.on_disconnected()) else {
            return;
        };
        deliver(deliveries, cx);
    }
}

fn deliver(deliveries: Vec<(Sink, TerminalMessage)>, cx: &mut AsyncApp) {
    if deliveries.is_empty() {
        return;
    }
    cx.update(|cx| {
        for (sink, message) in deliveries {
            sink(message, cx);
        }
    });
}

#[cfg(test)]
mod tests;

//! A running Muse session: the command handle, and the loop that turns MSP events into
//! [`AgentEvent`]s.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use burrow_msp::{
    ApprovalRequestParams, ErrorKind, Event, Events, ModelListResult, MspClient, MspError,
    SessionStartParams, TurnStartParams,
};
use burrow_protocol::{ContentBlock, ModelInfo};
use futures::future::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use super::translate::{self, Output, Translator, display_path};
use crate::adapter::{
    AgentError, AgentEvent, AgentSession, PermissionOutcome, PermissionReply, PermissionRequest,
    SessionConfig, StartedSession,
};
use crate::launcher::ProcessHandle;

/// How long a stopping Muse gets to unload its session and exit after its stdin closes.
pub(super) const STOP_GRACE: Duration = Duration::from_secs(5);

/// Approvals waiting for a decision, by approval id, as Muse last described them.
type Approvals = Arc<Mutex<HashMap<String, ApprovalRequestParams>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Opens a session on a connected Muse and starts translating its events. `process` is the
/// `muse serve` tree, or `None` when the connection isn't a process (tests).
pub(super) async fn open(
    client: MspClient,
    events: Events,
    process: Option<ProcessHandle>,
    config: &SessionConfig,
) -> Result<StartedSession, AgentError> {
    let workspace = display_path(&config.cwd.display().to_string());
    let mut params = SessionStartParams::new(workspace.clone());
    params.model_id.clone_from(&config.model);
    let session = client
        .start_session(&params)
        .await
        .map_err(|error| start_error(&error))?
        .session;
    let session_id = session.session_id.clone();
    info!(
        session = config.session_id,
        muse_session = session_id,
        provider = session.provider_id.as_deref().unwrap_or("-"),
        model = session.model_id.as_deref().unwrap_or("-"),
        "muse session started"
    );

    let models = match client.list_models(Some(&session_id)).await {
        Ok(list) => models(&list, session.provider_id.as_deref()),
        Err(error) => {
            debug!(%error, "model/list failed");
            Vec::new()
        }
    };
    let (sender, receiver) = mpsc::unbounded_channel();
    match client.read_usage().await {
        Ok(reading) => {
            let _ = sender.send(AgentEvent::ProviderUsage(translate::provider_usage(
                reading.as_ref(),
            )));
        }
        Err(error) => debug!(%error, "usage/read failed"),
    }

    let translator = Arc::new(Mutex::new(Translator::new(
        session_id.clone(),
        Path::new(&workspace),
    )));
    let stopping = Arc::new(AtomicBool::new(false));
    tokio::spawn(
        Loop {
            client: client.clone(),
            translator: translator.clone(),
            approvals: Approvals::default(),
            process: process.clone(),
            stopping: stopping.clone(),
            events: sender,
            closed: None,
        }
        .run(events),
    );
    Ok(StartedSession {
        session: Box::new(MspSession {
            client,
            session_id: session_id.clone(),
            translator,
            process,
            stopping,
        }),
        events: receiver,
        model: session.model_id,
        models,
        agent_session_id: Some(session_id),
    })
}

/// The models `session/setModel` accepts: the catalog's entries for the session's provider.
/// An `echo` session has none, though `model/list` still lists `meta`'s catalog.
pub(super) fn models(list: &ModelListResult, provider: Option<&str>) -> Vec<ModelInfo> {
    list.models
        .iter()
        .filter(|model| provider.is_none_or(|provider| model.provider_id == provider))
        .map(|model| ModelInfo {
            id: model.model_id.clone(),
            name: (model.display_label != model.model_id).then(|| model.display_label.clone()),
        })
        .collect()
}

fn start_error(error: &MspError) -> AgentError {
    match error.rpc() {
        Some(rpc) => {
            AgentError::Unavailable(format!("Muse refused to start a session: {}", rpc.message))
        }
        None => AgentError::Unavailable(format!("starting a Muse session failed: {error}")),
    }
}

/// A failed call, as an adapter error.
fn call_error(what: &str, error: MspError) -> AgentError {
    match (&error, error.kind()) {
        (MspError::Closed, _) => AgentError::InvalidState("Muse is shutting down".into()),
        (
            _,
            Some(
                ErrorKind::CommandRejected
                | ErrorKind::InvalidParams
                | ErrorKind::NotFound
                | ErrorKind::SessionNotLoaded
                | ErrorKind::Backpressured
                | ErrorKind::Overloaded,
            ),
        ) => AgentError::InvalidState(format!(
            "Muse refused {what}: {}",
            error.rpc().map_or("", |rpc| rpc.message.as_str())
        )),
        _ => AgentError::Failed(format!("{what} failed: {error}")),
    }
}

/// The command handle of a Muse session.
struct MspSession {
    client: MspClient,
    session_id: String,
    translator: Arc<Mutex<Translator>>,
    process: Option<ProcessHandle>,
    stopping: Arc<AtomicBool>,
}

impl Drop for MspSession {
    fn drop(&mut self) {
        // Dropped without `stop`, for example because burrowd is shutting down: kill the tree.
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(process) = &self.process {
            process.kill();
        }
    }
}

impl AgentSession for MspSession {
    fn prompt(&self, content: Vec<ContentBlock>) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let text: Vec<&str> = content.iter().filter_map(ContentBlock::as_text).collect();
            let params = TurnStartParams::text(&self.session_id, text.join("\n"));
            // A fresh turn's id is its commandId: expect it before Muse can send its events.
            lock(&self.translator).begin_turn(&params.command_id);
            match self.client.start_turn(&params).await {
                Ok(started) => {
                    if started.turn_id != params.command_id {
                        lock(&self.translator).begin_turn(started.turn_id);
                    }
                    Ok(())
                }
                Err(error) => {
                    lock(&self.translator).abandon_turn();
                    Err(call_error("the prompt", error))
                }
            }
        })
    }

    fn cancel(&self) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            let Some(turn) = lock(&self.translator).turn_id().map(str::to_owned) else {
                return Ok(());
            };
            match self
                .client
                .interrupt_turn(&self.session_id, Some(&turn))
                .await
            {
                Ok(_) => Ok(()),
                // `missing_run`: the turn is over already; its turn/completed ends it.
                Err(error) if error.kind() == Some(ErrorKind::CommandRejected) => {
                    debug!(%error, "nothing to interrupt");
                    Ok(())
                }
                Err(error) => Err(call_error("the cancel", error)),
            }
        })
    }

    fn set_model(&self, model: String) -> BoxFuture<'_, Result<(), AgentError>> {
        Box::pin(async move {
            self.client
                .set_model(&self.session_id, &model)
                .await
                .map(|_| ())
                .map_err(|error| call_error("the model", error))
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stopping.store(true, Ordering::SeqCst);
            // Closing stdin asks Muse to unload the session and exit; the job takes the rest.
            let _ = tokio::time::timeout(STOP_GRACE, self.client.shutdown()).await;
            if let Some(process) = &self.process {
                process.stop(STOP_GRACE).await;
            }
        })
    }
}

/// Reads one session's MSP events until the connection closes.
struct Loop {
    client: MspClient,
    translator: Arc<Mutex<Translator>>,
    approvals: Approvals,
    process: Option<ProcessHandle>,
    stopping: Arc<AtomicBool>,
    events: mpsc::UnboundedSender<AgentEvent>,
    /// Why the session ended, when Muse ended it.
    closed: Option<String>,
}

impl Loop {
    async fn run(mut self, mut events: Events) {
        while let Some(event) = events.recv().await {
            self.track_approvals(&event);
            let outputs = lock(&self.translator).on_event(event);
            for output in outputs {
                self.perform(output).await;
            }
        }

        // The connection closed: Muse exited, was stopped, or was killed.
        let error = if self.stopping.load(Ordering::SeqCst) {
            None
        } else if let Some(reason) = self.closed.take() {
            Some(reason)
        } else {
            Some(match &self.process {
                Some(process) => {
                    let exit = tokio::time::timeout(STOP_GRACE, process.exited()).await;
                    let exit = exit.map_or_else(|_| "stdout closed".to_owned(), |e| e.description);
                    match process.stderr_tail().last() {
                        Some(last) => format!("Muse exited ({exit}): {last}"),
                        None => format!("Muse exited ({exit})"),
                    }
                }
                None => "the connection to Muse closed".to_owned(),
            })
        };
        let _ = self.events.send(AgentEvent::Exited { error });
    }

    /// Keeps the approvals waiting for an answer as Muse last described them, so an answer
    /// decides the current requirement with a current choice.
    fn track_approvals(&self, event: &Event) {
        let mut approvals = lock(&self.approvals);
        match event {
            Event::ApprovalRequested(request) => {
                approvals.insert(request.approval_id.clone(), request.clone());
            }
            Event::ApprovalUpdated(update) => {
                if let Some(request) = approvals.get_mut(&update.approval_id) {
                    request
                        .available_choices
                        .clone_from(&update.available_choices);
                    request
                        .current_requirement_id
                        .clone_from(&update.current_requirement_id);
                    request.subject.clone_from(&update.subject);
                }
            }
            Event::ApprovalResolved(resolved) => {
                approvals.remove(&resolved.approval_id);
            }
            _ => {}
        }
    }

    async fn perform(&mut self, output: Output) {
        match output {
            Output::Event(event) => {
                match &event {
                    AgentEvent::LimitReached {
                        resets_at, message, ..
                    } => translate::remember_limit(*resets_at, message),
                    AgentEvent::TurnEnded {
                        reason: burrow_protocol::TurnEndReason::Completed,
                        ..
                    } if translate::forget_limit() => {
                        // A turn went through: the limit is over. Say so in usage/get.
                        if let Ok(reading) = self.client.read_usage().await {
                            let _ = self.events.send(AgentEvent::ProviderUsage(
                                translate::provider_usage(reading.as_ref()),
                            ));
                        }
                    }
                    _ => {}
                }
                let _ = self.events.send(event);
            }
            Output::Ask {
                tool_call_id,
                title,
                options,
                approval,
            } => {
                let (reply, answer) = PermissionReply::channel();
                tokio::spawn(decide(
                    self.client.clone(),
                    self.approvals.clone(),
                    *approval,
                    answer,
                ));
                let _ = self.events.send(AgentEvent::Permission(PermissionRequest {
                    tool_call_id,
                    title,
                    options,
                    reply,
                }));
            }
            Output::DeclineInput(request) => {
                warn!(
                    prompt = request.user_input_id,
                    "muse asked the user a question although it was started without user input; declining"
                );
                let client = self.client.clone();
                tokio::spawn(async move {
                    if let Err(error) = client.cancel_user_input(&request).await {
                        debug!(%error, "declining the question failed");
                    }
                });
            }
            Output::Closed(reason) => {
                warn!(reason, "muse ended the session");
                self.closed = Some(reason);
                match &self.process {
                    Some(process) => process.kill(),
                    None => self.client.shutdown().await,
                }
            }
        }
    }
}

/// Waits for the answer to an approval and passes it to Muse. Nobody answering (the turn was
/// cancelled, the session stopped) decides nothing: `turn/interrupt`, or Muse's exit, settles
/// the approval.
async fn decide(
    client: MspClient,
    approvals: Approvals,
    asked: ApprovalRequestParams,
    answer: oneshot::Receiver<PermissionOutcome>,
) {
    let Ok(PermissionOutcome::Selected(choice_id)) = answer.await else {
        debug!(approval = asked.approval_id, "an approval went unanswered");
        return;
    };
    let current = lock(&approvals).get(&asked.approval_id).cloned();
    let Some(current) = current else {
        warn!(
            approval = asked.approval_id,
            "the approval was settled before the answer arrived"
        );
        return;
    };
    let Some(choice) = translate::pick_choice(&asked, &current, &choice_id) else {
        warn!(
            approval = asked.approval_id,
            choice_id, "the answer isn't one of the approval's choices any more"
        );
        return;
    };
    match client.decide_approval(&current, &choice).await {
        Ok(decided) => debug!(
            approval = asked.approval_id,
            choice = choice.choice_id,
            terminal = decided.terminal,
            "approval decided"
        ),
        Err(error) => warn!(approval = asked.approval_id, %error, "approval/decide failed"),
    }
}

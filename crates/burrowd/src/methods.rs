//! Request dispatch for every method except the handshake and subscriptions, which
//! [`crate::connection`] handles itself because they change the connection.
//!
//! Each request runs on its own task (D-019) with a [`Cancel`] flag that `$/cancel` sets.
//! Requests that only read, or that can safely give up halfway (`agent/*`, `project/add`'s git
//! check, lists), answer `cancelled` as soon as the flag is set; `agent/install` kills npm and
//! leaves nothing half-installed. `session/create` gives up its
//! start and stops the session. Requests that change state and finish quickly answer normally.

use std::future::Future;
use std::sync::Arc;

use burrow_protocol::methods::{
    AgentInstall, AgentList, AgentRefresh, DebugAppend, InboxGet, InboxList, InboxRespond, Ping,
    ProjectAdd, ProjectGet, ProjectList, ProjectRemove, SessionArchive, SessionCancel,
    SessionCreate, SessionGet, SessionList, SessionPrompt, SessionSetAgentMode, SessionSetModel,
    SessionStop, UsageGet,
};
use burrow_protocol::{
    DebugAppendParams, DebugAppendResult, Empty, ErrorKind, ErrorObject, Method, Request, Response,
};
use tracing::debug;

use crate::cancel::Cancel;
use crate::connection::Context;
use crate::wire;

/// Who answers inbox items over a local connection.
const LOCAL: &str = "local";

/// Handles one request and returns its response.
pub async fn handle(context: &Arc<Context>, request: Request, cancel: Cancel) -> Response {
    let c = context;
    match request.method.as_str() {
        Ping::NAME => answer::<Ping, _, _>(&request, |_| async { Ok(Empty {}) }).await,
        // Development-only: without --dev it doesn't exist (protocol.md).
        DebugAppend::NAME if c.dev => {
            answer::<DebugAppend, _, _>(&request, |params| debug_append(c, params)).await
        }

        ProjectList::NAME => {
            answer::<ProjectList, _, _>(&request, |_| async { Ok(c.projects.list().await) }).await
        }
        ProjectAdd::NAME => {
            answer::<ProjectAdd, _, _>(&request, |params| {
                cancellable(&cancel, async move { c.projects.add(&params.path).await })
            })
            .await
        }
        ProjectGet::NAME => {
            answer::<ProjectGet, _, _>(&request, |params| async move {
                c.projects.get(&params.project_id).await
            })
            .await
        }
        ProjectRemove::NAME => {
            answer::<ProjectRemove, _, _>(&request, |params| async move {
                c.projects
                    .remove(&params.project_id)
                    .await
                    .map(|()| Empty {})
            })
            .await
        }

        AgentList::NAME => {
            answer::<AgentList, _, _>(&request, |_| {
                cancellable(&cancel, async { Ok(c.agents.list().await) })
            })
            .await
        }
        AgentRefresh::NAME => {
            answer::<AgentRefresh, _, _>(&request, |_| {
                cancellable(&cancel, async { Ok(c.agents.refresh().await) })
            })
            .await
        }
        AgentInstall::NAME => {
            answer::<AgentInstall, _, _>(&request, |params| {
                cancellable(
                    &cancel,
                    async move { c.agents.install(&params.agent).await },
                )
            })
            .await
        }
        UsageGet::NAME => {
            answer::<UsageGet, _, _>(&request, |_| async { Ok(c.agents.usage()) }).await
        }

        SessionList::NAME => {
            answer::<SessionList, _, _>(
                &request,
                |params| async move { Ok(c.sessions.list(&params)) },
            )
            .await
        }
        SessionCreate::NAME => {
            answer::<SessionCreate, _, _>(&request, |params| async {
                c.sessions.create(params, &cancel).await
            })
            .await
        }
        SessionGet::NAME => {
            answer::<SessionGet, _, _>(&request, |params| async move {
                c.sessions.get(&params.session_id)
            })
            .await
        }
        SessionPrompt::NAME => {
            answer::<SessionPrompt, _, _>(&request, |params| c.sessions.prompt(params)).await
        }
        SessionCancel::NAME => {
            answer::<SessionCancel, _, _>(&request, |params| async move {
                c.sessions
                    .cancel(&params.session_id)
                    .await
                    .map(|()| Empty {})
            })
            .await
        }
        SessionSetModel::NAME => {
            answer::<SessionSetModel, _, _>(&request, |params| async move {
                c.sessions.set_model(params).await.map(|()| Empty {})
            })
            .await
        }
        SessionSetAgentMode::NAME => {
            answer::<SessionSetAgentMode, _, _>(&request, |params| async move {
                c.sessions.set_agent_mode(params).await.map(|()| Empty {})
            })
            .await
        }
        SessionStop::NAME => {
            answer::<SessionStop, _, _>(&request, |params| async move {
                c.sessions.stop(&params.session_id).await.map(|()| Empty {})
            })
            .await
        }
        SessionArchive::NAME => {
            answer::<SessionArchive, _, _>(&request, |params| async move {
                c.sessions
                    .archive(&params.session_id)
                    .await
                    .map(|()| Empty {})
            })
            .await
        }

        InboxList::NAME => {
            answer::<InboxList, _, _>(&request, |params| async move {
                Ok(c.sessions.inbox_list(&params))
            })
            .await
        }
        InboxGet::NAME => {
            answer::<InboxGet, _, _>(
                &request,
                |params| async move { c.sessions.inbox_get(&params) },
            )
            .await
        }
        InboxRespond::NAME => {
            answer::<InboxRespond, _, _>(&request, |params| c.sessions.inbox_respond(params, LOCAL))
                .await
        }

        _ => Response::error(
            Some(request.id.clone()),
            ErrorObject::method_not_found(&request.method),
        ),
    }
}

/// Decodes the params of `M`, runs `run`, and encodes its result.
async fn answer<M, F, Fut>(request: &Request, run: F) -> Response
where
    M: Method,
    F: FnOnce(M::Params) -> Fut,
    Fut: Future<Output = Result<M::Result, ErrorObject>>,
{
    let result = match request.params_as::<M>() {
        Ok(params) => run(params).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(result) => Response::ok::<M>(request.id.clone(), &result),
        Err(error) => {
            debug!(method = M::NAME, %error, "request failed");
            Response::error(Some(request.id.clone()), error)
        }
    }
}

/// Runs `work`, giving up with `cancelled` as soon as `cancel` is set.
async fn cancellable<T>(
    cancel: &Cancel,
    work: impl Future<Output = Result<T, ErrorObject>>,
) -> Result<T, ErrorObject> {
    tokio::select! {
        result = work => result,
        () = cancel.cancelled() => Err(ErrorObject::new(ErrorKind::Cancelled, "the request was cancelled")),
    }
}

async fn debug_append(
    context: &Context,
    params: DebugAppendParams,
) -> Result<DebugAppendResult, ErrorObject> {
    wire::check_appendable(&params.stream, &params.event).map_err(ErrorObject::invalid_params)?;
    let record = wire::record(&params.event);
    let appended = context
        .hub
        .append(params.stream, vec![record])
        .await
        .map_err(|error| ErrorObject::internal(error.to_string()))?;
    Ok(DebugAppendResult {
        seq: *appended.seqs.start(),
    })
}

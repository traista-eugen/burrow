//! **Phase 2 slot:** the MSP adapter for Muse Code (`muse serve`).
//!
//! Until it lands, [`MspAdapter`] detects `muse` (see [`crate::detect`]) and reports it
//! unavailable; [`MspAdapter::start`] fails with [`AgentError::Unavailable`].
//!
//! # How phase 2 fills it in
//!
//! `start` should launch `muse serve` (found with [`detect::MUSE`]`.find()`; `--provider echo`
//! in tests) with [`launcher::spawn`](crate::launcher::spawn), hand its stdin and stdout to the
//! `burrow-msp` client, open a session in [`SessionConfig::cwd`], and map MSP's notifications to
//! [`AgentEvent`](crate::AgentEvent)s the same way the ACP adapter does (see [`crate::acp`]).
//! MSP's `usage/read` feeds [`AgentEvent::ProviderUsage`](crate::AgentEvent::ProviderUsage).

use burrow_protocol::{AgentId, AgentInfo, Integration};
use futures::future::BoxFuture;

use crate::adapter::{AgentAdapter, AgentError, SessionConfig, StartedSession};
use crate::detect;

/// Muse Code, through MSP. Not built yet: detection only.
#[derive(Clone, Copy, Debug, Default)]
pub struct MspAdapter;

impl MspAdapter {
    fn not_built(&self) -> String {
        "Burrow can't drive Muse Code yet: its MSP adapter is still being built (M2 phase 2)".into()
    }
}

impl AgentAdapter for MspAdapter {
    fn id(&self) -> AgentId {
        AgentId::Muse
    }

    fn name(&self) -> &str {
        detect::MUSE.name
    }

    fn integration(&self) -> Integration {
        Integration::Msp
    }

    fn detect(&self) -> BoxFuture<'_, AgentInfo> {
        Box::pin(async move {
            let mut info = detect::detect(&detect::MUSE).await;
            if info.installed {
                info.available = false;
                info.problem = Some(self.not_built());
            }
            info
        })
    }

    fn start(&self, _config: SessionConfig) -> BoxFuture<'_, Result<StartedSession, AgentError>> {
        Box::pin(async move { Err(AgentError::Unavailable(self.not_built())) })
    }
}

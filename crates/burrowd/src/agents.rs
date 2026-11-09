//! Agents: the adapter registry, cached detection results, and provider usage.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use burrow_agents::{AgentError, Registry};
use burrow_protocol::{AgentId, AgentInfo, ErrorKind, ErrorObject, ProviderUsage};
use tracing::{info, warn};

/// The agents `burrowd` offers, what was last detected about them, and the usage they reported.
#[derive(Debug)]
pub struct Agents {
    registry: Registry,
    /// The last detection. `None` until the first one finishes. Held while detecting, so
    /// concurrent callers share one detection.
    detected: tokio::sync::Mutex<Option<Vec<AgentInfo>>>,
    usage: Mutex<BTreeMap<String, ProviderUsage>>,
}

impl Agents {
    /// Agents from `registry`; nothing detected yet.
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            detected: tokio::sync::Mutex::new(None),
            usage: Mutex::new(BTreeMap::new()),
        }
    }

    /// The adapters.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Starts the first detection in the background, so `agent/list` answers quickly later.
    pub fn detect_in_background(self: &Arc<Self>) {
        let agents = self.clone();
        tokio::spawn(async move {
            agents.list().await;
        });
    }

    /// The cached detection, detecting first if nothing is cached.
    pub async fn list(&self) -> Vec<AgentInfo> {
        let mut detected = self.detected.lock().await;
        if let Some(agents) = detected.as_ref() {
            return agents.clone();
        }
        let agents = self.detect().await;
        *detected = Some(agents.clone());
        agents
    }

    /// Detects every agent again.
    pub async fn refresh(&self) -> Vec<AgentInfo> {
        let mut detected = self.detected.lock().await;
        let agents = self.detect().await;
        *detected = Some(agents.clone());
        agents
    }

    /// `agent/install`: installs what `agent` needs, such as its pinned ACP adapter, then
    /// detects it again and updates the cache.
    pub async fn install(&self, agent: &AgentId) -> Result<AgentInfo, ErrorObject> {
        let Some(adapter) = self.registry.get(agent) else {
            return Err(ErrorObject::new(
                ErrorKind::Unavailable,
                format!("agent {agent} isn't offered by this burrowd"),
            ));
        };
        info!(agent = %agent, "installing");
        if let Err(error) = adapter.install().await {
            warn!(agent = %agent, %error, "installing failed");
            return Err(match error {
                AgentError::Unavailable(message) => {
                    ErrorObject::new(ErrorKind::Unavailable, message)
                }
                AgentError::InvalidState(message) => {
                    ErrorObject::new(ErrorKind::InvalidState, message)
                }
                AgentError::Invalid(message) => ErrorObject::invalid_params(message),
                AgentError::LimitReached { message, resets_at } => {
                    ErrorObject::limit_reached(message, resets_at)
                }
                AgentError::Failed(message) => ErrorObject::internal(message),
            });
        }
        let info = adapter.detect().await;
        let mut detected = self.detected.lock().await;
        if let Some(agents) = detected.as_mut()
            && let Some(cached) = agents.iter_mut().find(|cached| cached.id == info.id)
        {
            *cached = info.clone();
        }
        info!(agent = %agent, available = info.available, "installed");
        Ok(info)
    }

    async fn detect(&self) -> Vec<AgentInfo> {
        let agents = self.registry.detect_all().await;
        for agent in &agents {
            info!(
                agent = %agent.id,
                installed = agent.installed,
                version = agent.version.as_deref().unwrap_or("-"),
                login = %agent.login.state,
                available = agent.available,
                problem = agent.problem.as_deref().unwrap_or("-"),
                "detected"
            );
        }
        agents
    }

    /// Records a provider's usage, replacing what it reported before.
    pub fn report_usage(&self, usage: ProviderUsage) {
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(usage.provider.clone(), usage);
    }

    /// Every provider's last reported usage, by provider name.
    pub fn usage(&self) -> Vec<ProviderUsage> {
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }
}

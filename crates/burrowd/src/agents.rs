//! Agents: the adapter registry, cached detection results, the models sessions offered, and
//! provider usage.
//!
//! `AgentInfo.models` are ids `session/create` accepts. Detection finds them where that's cheap
//! and the ids match a session's (Muse, the fakes). For the others, the models of the agent's
//! latest session are reported: an ACP agent only says which models it offers once a session
//! is open, and Cursor's `agent models` lists ids its sessions don't accept (D-032).
//! `AgentInfo.agentModes` are learned the same way, where detection doesn't know them (D-035).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use burrow_agents::{AgentError, Registry};
use burrow_protocol::{
    AgentId, AgentInfo, AgentMode, ErrorKind, ErrorObject, ModelInfo, ProviderUsage,
};
use tracing::{info, warn};

/// The agents `burrowd` offers, what was last detected about them, and the usage they reported.
#[derive(Debug)]
pub struct Agents {
    registry: Registry,
    /// The last detection. `None` until the first one finishes. Held while detecting, so
    /// concurrent callers share one detection.
    detected: tokio::sync::Mutex<Option<Vec<AgentInfo>>>,
    /// What each agent's latest session offered, by agent id.
    learned: Mutex<BTreeMap<String, Learned>>,
    usage: Mutex<BTreeMap<String, ProviderUsage>>,
}

/// What an agent's latest session offered.
#[derive(Debug, Default)]
struct Learned {
    models: Vec<ModelInfo>,
    agent_modes: Vec<AgentMode>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agents {
    /// Agents from `registry`; nothing detected yet.
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            detected: tokio::sync::Mutex::new(None),
            learned: Mutex::new(BTreeMap::new()),
            usage: Mutex::new(BTreeMap::new()),
        }
    }

    /// Records the models a session of `agent` offers, for `agent/list` (D-032).
    pub fn learn_models(&self, agent: &AgentId, models: Vec<ModelInfo>) {
        lock(&self.learned)
            .entry(agent.to_string())
            .or_default()
            .models = models;
    }

    /// Records the agent modes a session of `agent` offers, for `agent/list` (D-035).
    pub fn learn_agent_modes(&self, agent: &AgentId, modes: Vec<AgentMode>) {
        lock(&self.learned)
            .entry(agent.to_string())
            .or_default()
            .agent_modes = modes;
    }

    /// `info` with the learned models and modes where detection found none.
    fn with_models(&self, mut info: AgentInfo) -> AgentInfo {
        if let Some(learned) = lock(&self.learned).get(info.id.as_str()) {
            if info.models.is_empty() {
                info.models.clone_from(&learned.models);
            }
            if info.agent_modes.is_empty() {
                info.agent_modes.clone_from(&learned.agent_modes);
            }
        }
        info
    }

    fn all_with_models(&self, agents: Vec<AgentInfo>) -> Vec<AgentInfo> {
        agents
            .into_iter()
            .map(|info| self.with_models(info))
            .collect()
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
            return self.all_with_models(agents.clone());
        }
        let agents = self.detect().await;
        *detected = Some(agents.clone());
        self.all_with_models(agents)
    }

    /// Detects every agent again.
    pub async fn refresh(&self) -> Vec<AgentInfo> {
        let mut detected = self.detected.lock().await;
        let agents = self.detect().await;
        *detected = Some(agents.clone());
        self.all_with_models(agents)
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
        Ok(self.with_models(info))
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
        lock(&self.usage).insert(usage.provider.clone(), usage);
    }

    /// Every provider's last reported usage, by provider name.
    pub fn usage(&self) -> Vec<ProviderUsage> {
        lock(&self.usage).values().cloned().collect()
    }
}

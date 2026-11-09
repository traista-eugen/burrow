//! The set of agents `burrowd` offers.

use std::path::PathBuf;
use std::sync::Arc;

use burrow_protocol::{AgentId, AgentInfo};

use crate::acp::AcpAdapter;
use crate::adapter::AgentAdapter;
use crate::fake::FakeAdapter;
use crate::msp::MspAdapter;

/// The agents `burrowd` offers, in display order.
#[derive(Clone, Default)]
pub struct Registry {
    adapters: Vec<Arc<dyn AgentAdapter>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.adapters.iter().map(|adapter| adapter.id()))
            .finish()
    }
}

impl Registry {
    /// A registry of exactly these adapters.
    pub fn new(adapters: Vec<Arc<dyn AgentAdapter>>) -> Self {
        Self { adapters }
    }

    /// The standard adapters for `agents`, in that order. `fake` uses the fake agent at
    /// `fake_program`; ids without an adapter are skipped.
    pub fn standard(agents: &[AgentId], fake_program: Option<PathBuf>) -> Self {
        let adapters = agents
            .iter()
            .filter_map(|id| -> Option<Arc<dyn AgentAdapter>> {
                Some(match id {
                    AgentId::Claude => Arc::new(AcpAdapter::claude()),
                    AgentId::Codex => Arc::new(AcpAdapter::codex()),
                    AgentId::Cursor => Arc::new(AcpAdapter::cursor()),
                    AgentId::Muse => Arc::new(MspAdapter),
                    AgentId::Fake => Arc::new(FakeAdapter::new(fake_program.clone()?)),
                    AgentId::Other(_) => return None,
                })
            })
            .collect();
        Self { adapters }
    }

    /// The adapter for `id`.
    pub fn get(&self, id: &AgentId) -> Option<Arc<dyn AgentAdapter>> {
        self.adapters
            .iter()
            .find(|adapter| &adapter.id() == id)
            .cloned()
    }

    /// Every agent's id, in order.
    pub fn ids(&self) -> Vec<AgentId> {
        self.adapters.iter().map(|adapter| adapter.id()).collect()
    }

    /// Detects every agent at once, in registry order.
    pub async fn detect_all(&self) -> Vec<AgentInfo> {
        futures::future::join_all(self.adapters.iter().map(|adapter| adapter.detect())).await
    }
}

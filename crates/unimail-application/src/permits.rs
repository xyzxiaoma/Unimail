//! Runtime-neutral global and per-provider synchronization capacity.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use unimail_core::Provider;

/// RAII permit held for the complete claimed network operation.
pub trait SyncPermit: Send {}

/// Non-blocking shared capacity port. Runtimes may wake the coordinator when capacity returns.
pub trait SyncPermitPool: Send + Sync {
    fn try_acquire(&self, provider: Provider) -> Option<Box<dyn SyncPermit>>;
}

#[derive(Default)]
struct PermitState {
    global_used: usize,
    provider_used: HashMap<Provider, usize>,
}

/// Shared in-process gate enforcing both global and per-provider limits.
#[derive(Clone)]
pub struct BoundedSyncPermitPool {
    state: Arc<Mutex<PermitState>>,
    global_limit: usize,
    per_provider_limit: usize,
}

impl BoundedSyncPermitPool {
    /// Creates a capacity gate. Both limits must be non-zero.
    #[must_use]
    pub fn new(global_limit: usize, per_provider_limit: usize) -> Option<Self> {
        (global_limit > 0 && per_provider_limit > 0).then(|| Self {
            state: Arc::new(Mutex::new(PermitState::default())),
            global_limit,
            per_provider_limit,
        })
    }
}

impl SyncPermitPool for BoundedSyncPermitPool {
    fn try_acquire(&self, provider: Provider) -> Option<Box<dyn SyncPermit>> {
        let mut state = self.state.lock().ok()?;
        let provider_used = state.provider_used.get(&provider).copied().unwrap_or(0);
        if state.global_used >= self.global_limit || provider_used >= self.per_provider_limit {
            return None;
        }
        state.global_used += 1;
        state.provider_used.insert(provider, provider_used + 1);
        Some(Box::new(OwnedPermit {
            state: Arc::clone(&self.state),
            provider,
        }))
    }
}

struct OwnedPermit {
    state: Arc<Mutex<PermitState>>,
    provider: Provider,
}

impl SyncPermit for OwnedPermit {}

impl Drop for OwnedPermit {
    fn drop(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.global_used = state.global_used.saturating_sub(1);
        if let Some(used) = state.provider_used.get_mut(&self.provider) {
            *used = used.saturating_sub(1);
            if *used == 0 {
                state.provider_used.remove(&self.provider);
            }
        }
    }
}

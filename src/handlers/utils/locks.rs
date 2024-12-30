use std::fmt::Debug;
use std::sync::Arc;
use derive_more::Display;
use flurry::HashSet;
use crate::config::FeatureToggles;

use crate::handlers::utils::callbacks::CallbackDataWithPrefix;

// TODO: create a Redis based implementation
pub trait LockCallbackServiceImplTrait : Debug + Clone + Send + Sync {
    type Guard;
    
    fn try_lock<T>(&mut self, callback_data: &T) -> Option<Self::Guard> 
    where Self::Guard: Guard,
          T: CallbackDataWithPrefix;
}

pub trait Guard: Send + Sync {}

#[derive(Clone, Debug)]
pub enum LockCallbackServiceFacade {
    NoOp,
    InMemory(InMemoryLockCallbackService),
}

impl LockCallbackServiceFacade {
    pub fn from_config(features: FeatureToggles) -> Self {
        if features.pvp.callback_locks {
            log::info!("LockCallbackService: in-memory");
            Self::InMemory(InMemoryLockCallbackService::default())
        } else {
            log::info!("LockCallbackService: none");
            Self::NoOp
        }
    }

    pub fn try_lock<T>(&mut self, callback_data: &T) -> Option<Box<dyn Guard>>
    where T: CallbackDataWithPrefix,
    {
        match self {
            Self::NoOp => Some(Box::<NoOpGuard>::default()),
            Self::InMemory(service) => service.try_lock(callback_data)
                .map(|guard| Box::new(guard) as Box<dyn Guard>),
        }
    }
}

#[derive(Default)]
pub struct NoOpGuard {}
impl Guard for NoOpGuard {}

#[derive(Clone, Debug, Default)]
pub struct InMemoryLockCallbackService {
    inner_set: Arc<HashSet<String>>
}

impl LockCallbackServiceImplTrait for InMemoryLockCallbackService {
    type Guard = InMemorySetGuard;
    
    fn try_lock<T>(&mut self, callback_data: &T) -> Option<Self::Guard>
    where Self::Guard: Guard,
          T: CallbackDataWithPrefix
    {
        let key = callback_data.to_string();
        if self.inner_set.contains(&key, &self.inner_set.guard()) {
            log::debug!("double attack on: {key}");
            None
        } else {
            self.inner_set.insert(key.clone(), &self.inner_set.guard());
            Some(InMemorySetGuard::new(&self.inner_set, key))
        }
    }
}

#[derive(Debug, Display, Clone)]
#[display("InMemorySetGuard({key})")]
pub struct InMemorySetGuard {
    set_ref: Arc<HashSet<String>>,
    key: String
}

impl InMemorySetGuard {
    pub fn new(set_ref: &Arc<HashSet<String>>, key: String) -> Self {
        let set_ref = Arc::clone(set_ref);
        let guard = Self { set_ref, key };
        log::debug!("taking a lock guard: {guard}");
        guard
    }
}

impl Drop for InMemorySetGuard {
    fn drop(&mut self) {
        log::debug!("dropping the lock guard: {self}");
        self.set_ref.remove(&self.key, &self.set_ref.guard());
    }
}

impl Guard for InMemorySetGuard {}

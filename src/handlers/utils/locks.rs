use std::sync::Arc;
use flurry::HashSet;
use crate::config::FeatureToggles;

use crate::handlers::utils::callbacks::CallbackDataWithPrefix;

// TODO: create a Redis based implementation
pub trait LockCallbackServiceTrait<T> : Clone {
    fn try_lock(&self, callback_data: &T) -> bool;

    fn free_lock(&self, callback_data: T);
}

#[derive(Clone)]
pub enum LockCallbackServiceFacade {
    NoOp,
    InMemory(InMemoryLockCallbackService),
}

impl LockCallbackServiceFacade {
    pub fn from_config(features: FeatureToggles) -> Self {
        if features.pvp.callback_locks {
            Self::InMemory(InMemoryLockCallbackService::default())
        } else {
            Self::NoOp
        }
    }
}

impl <T: CallbackDataWithPrefix> LockCallbackServiceTrait<T> for LockCallbackServiceFacade {
    fn try_lock(&self, callback_data: &T) -> bool {
        match self {
            Self::NoOp => true,
            Self::InMemory(service) => service.try_lock(callback_data),
        }
    }

    fn free_lock(&self, callback_data: T) {
        match self {
            Self::NoOp => {},
            Self::InMemory(service) => service.free_lock(callback_data),
        }
    }
}

#[derive(Clone, Default)]
pub struct InMemoryLockCallbackService {
    inner_set: Arc<HashSet<String>>
}

impl <T: CallbackDataWithPrefix> LockCallbackServiceTrait<T> for InMemoryLockCallbackService {
    fn try_lock(&self, callback_data: &T) -> bool {
        let key = callback_data.to_string();
        let guard = self.inner_set.guard();
        if self.inner_set.contains(&key, &guard) {
            false
        } else {
            log::debug!("lock the message with a key: {key}");
            self.inner_set.insert(key, &guard);
            true
        }
    }

    fn free_lock(&self, callback_data: T) {
        let key = callback_data.to_string();
        let guard = self.inner_set.guard();
        self.inner_set.remove(&key, &guard);
        log::debug!("unlock the message with a key: {key}");
    }
}

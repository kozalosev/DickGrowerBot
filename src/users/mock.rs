use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use teloxide::types::UserId;
use tonic::Status;
use super::generated::User;
use super::{UserService, UserServiceClient};

/// In-memory test double for [`UserServiceClient`]. Pre-populate registered users with
/// [`Self::insert`]; users absent from the map behave as "not registered" (`get` → `None`,
/// `set_language` → `NotFound`), so tests can exercise the unregistered path too.
#[derive(Clone, Default)]
pub struct UserServiceClientMock {
    users: Arc<Mutex<HashMap<UserId, User>>>,
}

impl UserServiceClientMock {
    pub fn new() -> UserService<Self> {
        UserService::Connected(Self::default())
    }

    /// Registers a user so subsequent `get`/`set_language` calls succeed.
    pub fn insert(&self, uid: UserId, user: User) {
        self.users.lock().unwrap().insert(uid, user);
    }

    pub fn language_of(&self, uid: UserId) -> Option<String> {
        self.users.lock().unwrap()
            .get(&uid)
            .and_then(|u| u.options.as_ref())
            .and_then(|opts| opts.language_code.clone())
    }
}

impl UserServiceClient for UserServiceClientMock {
    async fn get(&self, uid: UserId) -> Result<Option<User>, Status> {
        Ok(self.users.lock().unwrap().get(&uid).cloned())
    }

    async fn set_language(&self, uid: UserId, code: &str) -> Result<(), Status> {
        let mut users = self.users.lock().unwrap();
        let user = users.get_mut(&uid).ok_or_else(|| Status::not_found("user"))?;
        let mut opts = user.options.take().unwrap_or_default();
        opts.language_code = Some(code.to_owned());
        user.options = Some(opts);
        Ok(())
    }
}

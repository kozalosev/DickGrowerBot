use std::ops::Deref;
use derive_more::{Constructor, From};

#[derive(Debug, Clone, Constructor, From, Eq, PartialEq)]
pub struct Username(String);

impl Username {
    pub fn value_ref(&self) -> &str {
        &self.0
    }
    
    pub fn value_clone(&self) -> String {
        self.0.clone()
    }

    pub fn escaped(&self) -> String {
        teloxide::utils::html::escape(self.value_ref())
    }
}

impl AsRef<String> for Username {
    fn as_ref(&self) -> &String {
        &self.0
    }
}

impl Deref for Username {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.as_str()
    }
}

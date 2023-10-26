mod users;
mod dicks;
mod imports;
mod promo;

use std::num::TryFromIntError;
use teloxide::types::ChatId;
pub use users::*;
pub use dicks::*;
pub use imports::*;
pub use promo::*;

#[derive(Clone)]
pub struct Repositories {
    pub users: Users,
    pub dicks: Dicks,
    pub imports: Imports,
    pub promo: Promo,
}

#[derive(Copy, Clone)]
pub struct UID(pub i64);

impl TryFrom<u64> for UID {
    type Error = TryFromIntError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        value.try_into()
            .map(UID)
    }
}

pub enum ChatIdKind {
    ID(ChatId),
    Instance(String)
}

impl From<ChatId> for ChatIdKind {
    fn from(value: ChatId) -> Self {
        ChatIdKind::ID(value)
    }
}

impl From<String> for ChatIdKind {
    fn from(value: String) -> Self {
        ChatIdKind::Instance(value)
    }
}

impl ChatIdKind {
    pub fn kind(&self) -> String {
        match self {
            ChatIdKind::ID(_) => "id",
            ChatIdKind::Instance(_) => "inst"
        }.to_owned()
    }

    pub fn value(&self) -> String {
        match self {
            ChatIdKind::ID(id) => id.0.to_string(),
            ChatIdKind::Instance(instance) => instance.to_owned()
        }
    }
}


#[macro_export]
macro_rules! repository {
    ($name:ident, $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<sqlx::Postgres>
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
                Self { pool }
            }

            $($methods)*
        }
    };
}

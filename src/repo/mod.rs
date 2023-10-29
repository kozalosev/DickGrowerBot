mod users;
mod dicks;
mod imports;
mod promo;

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
    pub fn value(&self) -> String {
        match self {
            ChatIdKind::ID(id) => id.0.to_string(),
            ChatIdKind::Instance(instance) => instance.to_owned()
        }
    }
}

#[derive(sqlx::Type)]
#[sqlx(type_name = "chat_id_type")]
#[sqlx(rename_all = "lowercase")]
enum ChatIdType {
    ID,
    Inst,
}

impl From<&ChatIdKind> for ChatIdType {
    fn from(value: &ChatIdKind) -> Self {
        match value {
            ChatIdKind::ID(_) => ChatIdType::ID,
            ChatIdKind::Instance(_) => ChatIdType::Inst,
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

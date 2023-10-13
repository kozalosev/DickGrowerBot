mod users;
mod dicks;
mod imports;

pub use users::*;
pub use dicks::*;
pub use imports::*;

#[derive(Clone)]
pub struct Repositories {
    pub users: Users,
    pub dicks: Dicks,
    pub imports: Imports,
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

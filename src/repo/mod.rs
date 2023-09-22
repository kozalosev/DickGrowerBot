mod users;
mod dicks;

pub use users::*;
pub use dicks::*;

#[macro_export]
macro_rules! repository {
    ($name:ident, $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<Postgres>
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<Postgres>) -> Self {
                Self { pool }
            }

            $($methods)*
        }
    };
}

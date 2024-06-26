pub trait UserNameAware {
    fn username(&self) -> &str;

    fn username_escaped(&self) -> String {
        teloxide::utils::html::escape(self.username())
    }
}

#[macro_export]
macro_rules! impl_username_aware {
    ($structName:ty) => {
        impl_username_aware!($structName, name);
    };
    ($structName:ty, $fieldName:tt) => {
        impl $crate::handlers::utils::username::UserNameAware for $structName {
            fn username(&self) -> &str {
                &self.$fieldName
            }
        }
    };
}

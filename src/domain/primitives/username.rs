use std::fmt::Display;
use std::str::FromStr;
use unicode_general_category::GeneralCategory::Format;
use unicode_general_category::get_general_category;
use domain_types_macro::domain_type;

const LTR_MARK: char = '\u{200E}';

#[domain_type(
    features(no_auto_display)
)]
pub struct Username(String);

impl Username {
    pub fn value_ref(&self) -> &str {
        &self.0
    }
    
    pub fn value_clone(&self) -> String {
        self.0.clone()
    }

    pub fn escaped(&self) -> String {
        let safe_name: String = self.value_ref().chars()
            .filter(|c| get_general_category(*c) != Format)
            .collect();
        let ltr_name = format!("{LTR_MARK}{safe_name}{LTR_MARK}");
        teloxide::utils::html::escape(&ltr_name)
    }

    pub fn value_with_at_sign(&self) -> String {
        if self.0.starts_with('@') {
            self.0.clone()
        } else {
            format!("@{}", self.0)
        }
    }
}

// TODO: move to the library code

#[derive(Debug, derive_more::Error, derive_more::Display)]
pub struct EmptyStringValue;

impl From<&str> for Username {
    fn from(value: &str) -> Self {
        Username::new(value.to_owned())
    }
}

impl FromStr for Username {
    type Err = EmptyStringValue;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Err(EmptyStringValue)
        } else {
            Ok(Username::from(s))
        }
    }
}

impl Display for Username {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value_with_at_sign())
    }
}

// TODO: ..end

#[cfg(test)]
mod test {
    use crate::domain::primitives::Username;

    #[test]
    fn test_username_value_with_at_sign() {
        let result = "@test";
        for variant in ["test", "@test"] {
            let username = Username::from(variant);
            assert_eq!(username.value_with_at_sign(), result);
        }
    }
}

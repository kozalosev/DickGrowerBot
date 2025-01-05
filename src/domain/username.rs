use std::ops::Deref;
use derive_more::{Constructor, From};
use unicode_general_category::GeneralCategory::Format;
use unicode_general_category::get_general_category;

const LTR_MARK: char = '\u{200E}';

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
        let safe_name: String = self.value_ref().chars()
            .filter(|c| get_general_category(*c) != Format)
            .collect();
        let ltr_name = format!("{LTR_MARK}{safe_name}{LTR_MARK}");
        teloxide::utils::html::escape(&ltr_name)
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

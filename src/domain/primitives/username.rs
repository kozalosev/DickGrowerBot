use unicode_general_category::GeneralCategory::Format;
use unicode_general_category::get_general_category;
use domain_types_macro::domain_type;

const LTR_MARK: char = '\u{200E}';

#[domain_type]
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

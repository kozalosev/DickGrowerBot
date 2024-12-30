use derive_more::{Constructor, From};

const LINE_LENGTH: usize = 60;

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

#[derive(Debug, Clone, Constructor, From, Eq, PartialEq)]
pub struct UserRealName {
    first_name: String,
    last_name: String,
}

impl UserRealName {
    pub fn shorten(&self, subtrahend: usize) -> String {
        let line_length = LINE_LENGTH - subtrahend;
        if self.first_name.len() + self.last_name.len() + 1 < line_length {
            format!("{} {}", self.first_name, self.last_name)
        } else if self.first_name.len() < line_length {
            self.first_name.clone()
        } else {
            self.first_name[..line_length].to_string()
        }
    }
}

impl std::fmt::Display for UserRealName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl AsRef<String> for UserRealName {
    fn as_ref(&self) -> &String {
        &self.0
    }
}
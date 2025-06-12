use std::collections::HashMap;
use std::ops::Not;
use std::sync::Arc;
use sha2::{Digest, Sha256};
use crate::domain::primitives::{Counter, TextHash, LanguageCode, SupportedLanguage};

#[derive(Clone, Default)]
pub struct AnnouncementsConfig {
    pub max_shows: Counter,
    pub announcements: HashMap<SupportedLanguage, Announcement>,
}

impl AnnouncementsConfig {
    pub fn get(&self, lang_code: &LanguageCode) -> Option<&Announcement> {
        self.announcements.get(&lang_code.to_supported_language())
    }
}

#[derive(Clone)]
pub struct Announcement {
    pub text: Arc<String>,
    pub hash: Arc<TextHash>,
}

impl Announcement {
    pub fn new(text: String) -> Option<Self> {
        text.is_empty().not().then(|| Self  {
            hash: Arc::new(TextHash::new(Sha256::digest(text.as_bytes()).to_vec())),
            text: Arc::new(text),
        })
    }
}

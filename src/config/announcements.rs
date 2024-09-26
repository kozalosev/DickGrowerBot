use std::collections::HashMap;
use std::ops::Not;
use std::sync::Arc;
use sha2::{Digest, Sha256};
use sha2::digest::core_api::CoreWrapper;
use crate::domain::{LanguageCode, SupportedLanguage};

#[derive(Clone, Default)]
pub struct AnnouncementsConfig {
    pub max_shows: usize,
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
    pub hash: Arc<Vec<u8>>,
}

impl Announcement {
    pub(super) fn new(text: String) -> Option<Self> {
        text.is_empty().not().then(|| Self  {
            hash: Arc::new(hash(&text)),
            text: Arc::new(text),
        })
    }
}

fn hash(s: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    CoreWrapper::update(&mut hasher, s.as_bytes());
    (*hasher.finalize()).to_vec()
}

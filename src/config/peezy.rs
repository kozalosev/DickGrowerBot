use teloxide::types::ChatId;

#[derive(Clone)]
pub struct PeezyForkSettings {
    pub allowed_chat_id: ChatId,
    pub centimeters_per_eggplant: i32,
    pub max_eggplants: usize,
}

#[cfg(test)]
impl Default for PeezyForkSettings {
    fn default() -> Self {
        Self {
            allowed_chat_id: ChatId(123456789),
            centimeters_per_eggplant: Default::default(),
            max_eggplants: Default::default(),
        }
    }
}

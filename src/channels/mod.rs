//! Chat platforms, normalised to `Envelope`.

pub mod telegram;

#[derive(Debug, Clone)]
pub struct Envelope {
    pub platform: &'static str,
    pub chat_id: String,
    /// Platform-native numeric user id, as a string.
    pub sender_id: String,
    pub sender_name: String,
    pub sender_is_bot: bool,
    pub is_group: bool,
    pub text: String,
}

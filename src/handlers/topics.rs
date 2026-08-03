use autometrics::autometrics;
use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{CallbackQuery, Message, UserId};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use crate::{check_invoked_by_owner_and_get_answer_params, reply_html};
use crate::domain::objects::AllowedTopics;
use crate::domain::primitives::LanguageCode;
use crate::domain::primitives::chat::{ChatIdPartiality, TopicId};
use crate::handlers::{reply_html, HandlerDeps, HandlerResult};
use crate::handlers::utils::{callbacks, is_chat_admin, is_forum};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::metrics;
use crate::topics::TopicPolicy;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum TopicsCommands {
    #[command(description = "topics")]
    Topics,
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn topics_cmd_handler(
    bot: Bot,
    msg: Message,
    topics: TopicPolicy,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_TOPICS.invoked();

    if !is_forum(&msg.chat) {
        reply_html!(bot, msg, t!("commands.topics.errors.not_a_forum", locale = &lang_code));
        return Ok(());
    }
    let from_id = msg.from.as_ref().map(|user| user.id)
        .ok_or(anyhow!("unexpected absence of a FROM field"))?;
    if !is_chat_admin(&bot, &msg, from_id).await? {
        reply_html!(bot, msg, t!("commands.topics.errors.admins_only", locale = &lang_code));
        return Ok(());
    }

    let current = TopicId::from(msg.thread_id);
    let allowed = topics.allowed(&msg.chat.id.into()).await;
    let keyboard = build_keyboard(&allowed, current, from_id, &lang_code);
    reply_html(bot, &msg, render_state(&allowed, current, &lang_code))
        .reply_markup(ReplyMarkup::InlineKeyboard(keyboard))
        .await?;
    Ok(())
}

#[inline]
pub fn callback_filter(query: CallbackQuery) -> bool {
    TopicsCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn topics_callback_handler(
    bot: Bot,
    query: CallbackQuery,
    topics: TopicPolicy,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let data = TopicsCallbackData::parse(&query)?;
    let answer = check_invoked_by_owner_and_get_answer_params!(bot, query, data.uid);
    let edit_msg_params = callbacks::get_params_for_message_edit(&query)?;

    let message = query.message.as_ref()
        .ok_or(anyhow!("a topics callback without an attached message"))?;
    let chat_id: ChatIdPartiality = message.chat().id.into();

    let allowed = match data.action {
        TopicsAction::Allow(topic) => topics.allow_topic(&chat_id, topic).await?,
        TopicsAction::Forbid(topic) => topics.forbid_topic(&chat_id, topic).await?,
        TopicsAction::AllowAll => topics.allow_all_topics(&chat_id).await?,
    };
    metrics::CMD_TOPICS.finished();

    let current = data.current;
    let keyboard = build_keyboard(&allowed, current, data.uid, &lang_code);
    callbacks::edit_message_text_with_keyboard(&bot, edit_msg_params,
        render_state(&allowed, current, &lang_code), Some(keyboard)).await?;
    answer.await?;
    Ok(())
}

/// The line above the buttons: whether the bot works in the topic the picker was opened in, and
/// how many topics it is confined to overall.
///
/// The other topics can't be named — the Bot API doesn't tell us their names — so they are a count
/// rather than a list, and each one is managed from inside itself.
fn render_state(allowed: &AllowedTopics, current: TopicId, lang_code: &LanguageCode) -> String {
    if allowed.is_unrestricted() {
        return t!("commands.topics.state.unrestricted", locale = lang_code).to_string()
    }
    let key = if allowed.allows(current) {
        "commands.topics.state.allowed_here"
    } else {
        "commands.topics.state.forbidden_here"
    };
    t!(key, locale = lang_code, count = allowed.count()).to_string()
}

/// The picker: what to do with the topic the command was invoked in, and how to lift the
/// restriction altogether. One button per row — the labels name what they act on rather than
/// saying "here", and two of those side by side would be cut off.
fn build_keyboard(
    allowed: &AllowedTopics,
    current: TopicId,
    uid: UserId,
    lang_code: &LanguageCode,
) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();

    // Three states, three labels. In an unrestricted chat the press *narrows* the bot down to this
    // topic — calling that "allow here" would offer to permit what is already permitted.
    let (label, action) = if allowed.is_unrestricted() {
        ("commands.topics.buttons.only_here", TopicsAction::Allow(current))
    } else if allowed.allows(current) {
        ("commands.topics.buttons.forbid_here", TopicsAction::Forbid(current))
    } else {
        ("commands.topics.buttons.allow_here", TopicsAction::Allow(current))
    };
    let data = TopicsCallbackData { uid, current, action };
    rows.push(vec![InlineKeyboardButton::callback(
        t!(label, locale = lang_code).to_string(),
        data.to_data_string())]);

    // Only a chat that has a restriction can be offered to lift it.
    if !allowed.is_unrestricted() {
        let data = TopicsCallbackData {
            uid, current, action: TopicsAction::AllowAll,
        };
        rows.push(vec![InlineKeyboardButton::callback(
            t!("commands.topics.buttons.allow_everywhere", locale = lang_code).to_string(),
            data.to_data_string())]);
    }

    InlineKeyboardMarkup::new(rows)
}

#[derive(Clone, Copy, PartialEq, Eq, derive_more::Display)]
#[cfg_attr(test, derive(Debug))]
enum TopicsAction {
    #[display("a:{_0}")]
    Allow(TopicId),
    #[display("f:{_0}")]
    Forbid(TopicId),
    #[display("all")]
    AllowAll,
}

/// Callback payload of the topic picker. The wire format is
/// `topics:<uid>:<current>:<a|f|all>[:<topic>]`: `uid` is the invoker (checked on press),
/// `current` is the topic the picker was opened in (so the keyboard and the status line can be
/// rebuilt around it), and the rest is the action.
#[derive(derive_more::Display)]
#[display("{uid}:{current}:{action}")]
pub struct TopicsCallbackData {
    uid: UserId,
    current: TopicId,
    action: TopicsAction,
}

impl CallbackDataWithPrefix for TopicsCallbackData {
    fn prefix() -> &'static str {
        "topics"
    }
}

impl TryFrom<String> for TopicsCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.as_str().split(':');
        let uid = callbacks::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let current: i32 = callbacks::parse_part(&mut parts, &err, "current")?;
        let action = match parts.next().ok_or_else(|| err.missing_part("action"))? {
            "a" => TopicsAction::Allow(TopicId::new(callbacks::parse_part(&mut parts, &err, "topic")?)),
            "f" => TopicsAction::Forbid(TopicId::new(callbacks::parse_part(&mut parts, &err, "topic")?)),
            "all" => TopicsAction::AllowAll,
            _ => return Err(err.split_err()),
        };
        Ok(Self { uid, current: TopicId::new(current), action })
    }
}

#[cfg(test)]
mod test {
    use teloxide::types::{InlineKeyboardButtonKind, UserId};
    use crate::domain::objects::AllowedTopics;
    use crate::domain::primitives::LanguageCode;
    use crate::domain::primitives::chat::TopicId;
    use crate::handlers::topics::{build_keyboard, TopicsAction, TopicsCallbackData};
    use crate::handlers::utils::callbacks::{build_callback_query, CallbackDataWithPrefix};

    fn round_trip(action: TopicsAction) -> TopicsCallbackData {
        let data = TopicsCallbackData {
            uid: UserId(12345),
            current: TopicId::new(7),
            action,
        };
        let query = build_callback_query(data.to_data_string());
        TopicsCallbackData::parse(&query).expect("couldn't parse the callback data")
    }

    /// The buttons of every picker already sent carry this exact layout, so it is a contract with
    /// the past, not an implementation detail — a `Display` that renders it differently would
    /// answer those presses with "invalid data".
    #[test]
    fn test_callback_data_wire_format() {
        let data = TopicsCallbackData {
            uid: UserId(12345),
            current: TopicId::new(7),
            action: TopicsAction::Allow(TopicId::new(42)),
        };
        assert_eq!(data.to_data_string(), "topics:12345:7:a:42");

        let data = TopicsCallbackData {
            uid: UserId(12345),
            current: TopicId::GENERAL,
            action: TopicsAction::AllowAll,
        };
        assert_eq!(data.to_data_string(), "topics:12345:0:all");
    }

    /// The payloads the picker offers, in order. Language-independent, unlike the labels.
    fn keyboard_actions(allowed: &AllowedTopics, current: TopicId) -> Vec<String> {
        build_keyboard(allowed, current, UserId(1), &LanguageCode::new("en".to_owned()))
            .inline_keyboard
            .into_iter()
            .flatten()
            .filter_map(|button| match button.kind {
                InlineKeyboardButtonKind::CallbackData(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    fn allowed_topics(ids: &[i32]) -> AllowedTopics {
        AllowedTopics::new(ids.iter().map(|id| TopicId::new(*id)).collect())
    }

    /// Lifting a restriction is only on offer when there is one to lift.
    #[test]
    fn test_the_unrestricted_chat_is_only_offered_to_narrow_down() {
        let actions = keyboard_actions(&AllowedTopics::default(), TopicId::new(7));
        assert_eq!(actions, vec!["topics:1:7:a:7"]);
    }

    #[test]
    fn test_a_restricted_chat_can_always_lift_the_restriction() {
        // standing in an allowed topic: forbid it, or drop the restriction
        let actions = keyboard_actions(&allowed_topics(&[7, 42]), TopicId::new(7));
        assert_eq!(actions, vec!["topics:1:7:f:7", "topics:1:7:all"]);

        // standing in a forbidden one: allow it, or drop the restriction
        let actions = keyboard_actions(&allowed_topics(&[42]), TopicId::new(7));
        assert_eq!(actions, vec!["topics:1:7:a:7", "topics:1:7:all"]);
    }

    #[test]
    fn test_callback_data_round_trip() {
        for action in [TopicsAction::Allow(TopicId::new(42)),
                       TopicsAction::Forbid(TopicId::GENERAL),
                       TopicsAction::AllowAll] {
            let parsed = round_trip(action);
            assert_eq!(parsed.uid, UserId(12345));
            assert_eq!(parsed.current, TopicId::new(7));
            assert_eq!(parsed.action, action);
        }
    }
}

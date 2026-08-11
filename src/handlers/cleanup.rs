use std::time::Duration;
use std::vec as row;
use autometrics::autometrics;
use anyhow::anyhow;
use itertools::Itertools;
use rust_i18n::t;
use strum::IntoEnumIterator;
use teloxide::{Bot, RequestError};
use teloxide::macros::BotCommands;
use teloxide::prelude::{CallbackQuery, Message, UserId};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use crate::{check_invoked_by_owner_and_get_answer_params, reply_html, reply_html_ephemeral};
use crate::cleanup::CleanupPolicy;
use crate::config::{DeletionMode, MessageGroup, SelfDestructionConfig};
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::{DelayMinutes, LanguageCode};
use crate::domain::primitives::chat::ChatIdPartiality;
use crate::handlers::{reply_html, HandlerDeps, HandlerResult};
use crate::handlers::utils::{callbacks, is_chat_admin};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, EditMessageReqParamsKind, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::metrics;

/// Three fit a row at every language's button widths; a fourth starts wrapping the labels.
const DELAYS_PER_ROW: usize = 3;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum CleanupCommands {
    #[command(description = "cleanup")]
    Cleanup,
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn cleanup_cmd_handler(
    bot: Bot,
    msg: Message,
    cleanup: CleanupPolicy,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, config, self_destruction, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_CLEANUP.invoked();

    let config = config.self_destruction;
    // Nothing self-destructs under this mode, so the refusal can't either.
    if !config.configurable() {
        reply_html!(bot, msg, t!("errors.feature_disabled", locale = &lang_code));
        return Ok(());
    }
    let from_id = msg.from.as_ref().map(|user| user.id)
        .ok_or(anyhow!("unexpected absence of a FROM field"))?;
    if !is_chat_admin(&bot, &msg, from_id).await? {
        reply_html_ephemeral!(bot, msg, t!("commands.cleanup.errors.admins_only", locale = &lang_code),
            self_destruction, MessageGroup::Notice, lang_code);
        return Ok(());
    }

    let settings = cleanup.settings(&msg.chat.id.into()).await;
    let button = ButtonBuilder { uid: from_id, lang_code: &lang_code };
    let screen = overview(&config, &settings, &button);
    reply_html_ephemeral!(bot, msg, screen.text,
        self_destruction, MessageGroup::Application, lang_code,
        reply_markup = ReplyMarkup::InlineKeyboard(screen.keyboard));
    Ok(())
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    CleanupCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn cleanup_callback_handler(
    bot: Bot,
    query: CallbackQuery,
    cleanup: CleanupPolicy,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, config, self_destruction, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let data = CleanupCallbackData::parse(&query)?;
    let answer = check_invoked_by_owner_and_get_answer_params!(bot, query, data.uid);
    let edit_msg_params = callbacks::get_params_for_message_edit(&query)?;

    let message = query.message.as_ref()
        .ok_or(anyhow!("a cleanup callback without an attached message"))?;
    let chat = message.chat().id;
    let chat_id: ChatIdPartiality = chat.into();
    let config = config.self_destruction;
    let button = ButtonBuilder { uid: data.uid, lang_code: &lang_code };

    // Opening the delay list changes nothing yet, so it never touches the database.
    if let CleanupAction::Pick(group) = data.action {
        delays(&config, group, &button).edit(&bot, edit_msg_params).await?;
        answer.await?;
        return Ok(());
    }

    // Asking for a group to be cleaned up where the commands can't go with the answers changes
    // what the chat gets rather than merely how soon, so it is shown and confirmed instead of
    // being done quietly. Keeping a group, or handing it back to the bot, takes nothing away.
    if let CleanupAction::Set(group, minutes) = data.action
        && !minutes.is_zero()
        && config.deletes_commands()
        && !self_destruction.may_delete_here(&bot, chat).await
    {
        warning(&config, group, minutes, &button).edit(&bot, edit_msg_params).await?;
        answer.await?;
        return Ok(());
    }

    let settings = match data.action {
        CleanupAction::Set(group, minutes) | CleanupAction::SetConfirmed(group, minutes) =>
            cleanup.set_group(&chat_id, group, minutes).await?,
        CleanupAction::Follow(group) => cleanup.follow_the_bot(&chat_id, group).await?,
        CleanupAction::Inline(compress) => cleanup.set_inline(&chat_id, compress).await?,
        CleanupAction::Reset => cleanup.reset(&chat_id).await?,
        CleanupAction::Pick(_) | CleanupAction::Back => cleanup.settings(&chat_id.kind()).await,
    };
    if !matches!(data.action, CleanupAction::Back) {
        metrics::CMD_CLEANUP.finished();
    }

    overview(&config, &settings, &button).edit(&bot, edit_msg_params).await?;
    answer.await?;
    Ok(())
}

/// What the picker shows: the text and the buttons under it. The two are built together so that a
/// keyboard can't end up under the text of another screen — nothing else would catch that.
struct Screen {
    text: String,
    keyboard: InlineKeyboardMarkup,
}

impl Screen {
    /// Puts the screen in place of the message a press came from.
    async fn edit(self, bot: &Bot, params: EditMessageReqParamsKind) -> Result<(), RequestError> {
        callbacks::edit_message_text_with_keyboard(bot, params, self.text, Some(self.keyboard)).await
    }
}

/// The first level: what happens to each kind of message in this chat and how soon, a button per
/// group opening its delay list, and a way back to the bot's own settings for a chat that changed
/// its mind about all of it.
fn overview(
    config: &SelfDestructionConfig,
    settings: &ChatCleanupSettings,
    button: &ButtonBuilder,
) -> Screen {
    let lang_code = button.lang();
    let (lines, rows): (Vec<String>, Vec<_>) = MessageGroup::iter()
        .map(|group| {
            let name = group_name(group, lang_code);
            let delay = config.delay_for_chat(group, settings);
            let label = group_label(&name, delay, lang_code);
            let button = button.with_label(label, CleanupAction::Pick(group));
            (state_line(&name, delay, lang_code), row![button])
        })
        .unzip();

    // An inline message can only be rewritten, never deleted, so it gets a switch of its own
    // instead of following the delays quietly.
    let compressed = config.compresses_inline_for_chat(settings);
    let inline = row![button.of(
        if compressed { ButtonKind::InlineOff } else { ButtonKind::InlineOn },
        CleanupAction::Inline(!compressed))];

    // Only a chat that decided something of its own can be offered to take it back.
    let take_back = (!settings.is_default())
        .then(|| row![button.of(ButtonKind::Reset, CleanupAction::Reset)]);

    let header = t!("commands.cleanup.state.header", locale = lang_code);
    let inline_line = t!(if compressed { "commands.cleanup.state.inline_compressed" }
        else { "commands.cleanup.state.inline_kept" }, locale = lang_code);
    let lines = lines.join("\n");
    Screen {
        text: format!("{header}\n\n{lines}\n\n{inline_line}"),
        keyboard: InlineKeyboardMarkup::new(rows.into_iter().chain([inline]).chain(take_back)),
    }
}

/// The second level: the delays the bot suggests for one group, three to a row, and the two answers
/// it always takes — keep them, or let the bot decide again.
fn delays(config: &SelfDestructionConfig, group: MessageGroup, button: &ButtonBuilder) -> Screen {
    let lang_code = button.lang();
    let suggested = config.delay_options.iter()
        .map(|minutes| button.with_label(
            format_delay(Duration::from_secs(u64::from(minutes.value()) * 60), lang_code),
            CleanupAction::Set(group, minutes)))
        .chunks(DELAYS_PER_ROW);
    let always = [
        row![button.of(ButtonKind::Keep, CleanupAction::Set(group, DelayMinutes::new(0)))],
        row![button.of(ButtonKind::Follow, CleanupAction::Follow(group))],
        row![button.of(ButtonKind::Back, CleanupAction::Back)],
    ];

    let rows: Vec<Vec<InlineKeyboardButton>> = suggested.into_iter()
        .map(Iterator::collect)
        .chain(always)
        .collect();
    Screen {
        text: t!("commands.cleanup.pick.prompt", locale = lang_code,
            group = group_name(group, lang_code)).to_string(),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

/// What an admin sees instead of the change when the bot may not delete the members' commands: the
/// answers would go and the commands stay behind, which is worse than a busy chat for some. The
/// delay that was picked travels through the detour on the confirming button.
fn warning(
    config: &SelfDestructionConfig,
    group: MessageGroup,
    minutes: DelayMinutes,
    button: &ButtonBuilder,
) -> Screen {
    let lang_code = button.lang();
    let key = if let DeletionMode::OnlyWithCommand = config.mode {
        "commands.cleanup.warning.no_rights_strict"
    } else {
        "commands.cleanup.warning.no_rights"
    };
    Screen {
        text: t!(key, locale = lang_code, group = group_name(group, lang_code)).to_string(),
        keyboard: InlineKeyboardMarkup::new([
            row![button.of(ButtonKind::Confirm, CleanupAction::SetConfirmed(group, minutes))],
            row![button.of(ButtonKind::Back, CleanupAction::Back)],
        ]),
    }
}

/// A line of the overview: what becomes of one group's messages in this chat.
fn state_line(name: &str, delay: Option<Duration>, lang_code: &LanguageCode) -> String {
    match delay {
        Some(delay) => t!("commands.cleanup.state.cleaned", locale = lang_code,
            group = name, delay = format_delay(delay, lang_code)),
        None => t!("commands.cleanup.state.kept", locale = lang_code, group = name),
    }.to_string()
}

/// The label of the button that opens a group's delay list: the group and what it has now.
fn group_label(name: &str, delay: Option<Duration>, lang_code: &LanguageCode) -> String {
    let delay = match delay {
        Some(delay) => format_delay(delay, lang_code),
        None => t!("commands.cleanup.delays.kept", locale = lang_code).to_string(),
    };
    t!("commands.cleanup.buttons.group", locale = lang_code, group = name, delay = delay).to_string()
}

/// A delay as an admin reads it: whole hours where it divides, minutes otherwise. The list is
/// written in minutes, so an hour would otherwise appear on a button as "60 min".
fn format_delay(delay: Duration, lang_code: &LanguageCode) -> String {
    let minutes = delay.as_secs() / 60;
    if minutes >= 60 && minutes.is_multiple_of(60) {
        t!("commands.cleanup.delays.hours", locale = lang_code, count = minutes / 60).to_string()
    } else {
        t!("commands.cleanup.delays.minutes", locale = lang_code, count = minutes).to_string()
    }
}

fn group_name(group: MessageGroup, lang_code: &LanguageCode) -> String {
    let key = format!("commands.cleanup.groups.{group}");
    t!(&key, locale = lang_code).to_string()
}

/// The buttons whose label is a fixed line of the locale file rather than something built from the
/// chat's settings. Each one is named after its key under `commands.cleanup.buttons`, so a label
/// that doesn't exist can't be asked for.
#[derive(Clone, Copy, strum_macros::Display, strum_macros::EnumIter)]
#[strum(serialize_all = "snake_case")]
enum ButtonKind {
    /// Keep this group for good.
    Keep,
    /// Hand this group back to the bot's own configuration.
    Follow,
    /// Return to the list of groups.
    Back,
    /// Start shrinking the inline messages to the placeholder.
    InlineOn,
    /// Stop touching the inline messages.
    InlineOff,
    /// Hand every group back at once.
    Reset,
    /// Set the delay after the warning about the commands the bot may not delete.
    Confirm,
}

/// Builds the buttons of one keyboard. The admin who opened the picker and the language it speaks
/// are the same for every button of it, so they are held here instead of being handed to each one.
struct ButtonBuilder<'a> {
    uid: UserId,
    lang_code: &'a LanguageCode,
}

impl ButtonBuilder<'_> {
    /// A button labeled by its kind.
    fn of(&self, kind: ButtonKind, action: CleanupAction) -> InlineKeyboardButton {
        let key = format!("commands.cleanup.buttons.{kind}");
        self.with_label(t!(&key, locale = self.lang_code), action)
    }

    /// The same for the two buttons whose text is built rather than looked up — a group with its
    /// current delay, and a delay on its own. Whatever it says, a button carries an action back
    /// together with the id of the admin, which is what the press is checked against.
    fn with_label(&self, label: impl Into<String>, action: CleanupAction) -> InlineKeyboardButton {
        let data = CleanupCallbackData { uid: self.uid, action };
        InlineKeyboardButton::callback(label, data.to_data_string())
    }

    /// The language the whole picker speaks — the texts of a screen come from the same place its
    /// labels do.
    fn lang(&self) -> &LanguageCode {
        self.lang_code
    }
}

#[derive(Clone, Copy, PartialEq, Eq, derive_more::Display)]
#[cfg_attr(test, derive(Debug))]
enum CleanupAction {
    #[display("pick:{_0}")]
    Pick(MessageGroup),
    #[display("set:{_0}:{_1}")]
    Set(MessageGroup, DelayMinutes),
    #[display("setc:{_0}:{_1}")]
    SetConfirmed(MessageGroup, DelayMinutes),
    #[display("follow:{_0}")]
    Follow(MessageGroup),
    #[display("inline:{}", if *_0 { "on" } else { "off" })]
    Inline(bool),
    #[display("reset")]
    Reset,
    #[display("back")]
    Back,
}

/// Callback payload of the cleanup picker. The wire format is
/// `cleanup:<uid>:<pick|set|setc|follow|reset|back>[:<group>[:<minutes>]]`: `uid` is the invoker
/// (checked on press), and the rest is the action. `setc` is the press that has already seen the
/// warning about the commands the bot may not delete, and it carries the same delay the press
/// before it did.
#[derive(derive_more::Display)]
#[display("{uid}:{action}")]
pub struct CleanupCallbackData {
    uid: UserId,
    action: CleanupAction,
}

impl CallbackDataWithPrefix for CleanupCallbackData {
    fn prefix() -> &'static str {
        "cleanup"
    }
}

impl TryFrom<String> for CleanupCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.as_str().split(':');
        let uid = callbacks::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let action = match parts.next().ok_or_else(|| err.missing_part("action"))? {
            "pick" => CleanupAction::Pick(callbacks::parse_part(&mut parts, &err, "group")?),
            "set" => CleanupAction::Set(
                callbacks::parse_part(&mut parts, &err, "group")?,
                callbacks::parse_part(&mut parts, &err, "minutes")?),
            "setc" => CleanupAction::SetConfirmed(
                callbacks::parse_part(&mut parts, &err, "group")?,
                callbacks::parse_part(&mut parts, &err, "minutes")?),
            "follow" => CleanupAction::Follow(callbacks::parse_part(&mut parts, &err, "group")?),
            "inline" => match parts.next().ok_or_else(|| err.missing_part("state"))? {
                "on" => CleanupAction::Inline(true),
                "off" => CleanupAction::Inline(false),
                _ => return Err(err.split_err()),
            },
            "reset" => CleanupAction::Reset,
            "back" => CleanupAction::Back,
            _ => return Err(err.split_err()),
        };
        Ok(Self { uid, action })
    }
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;
    use std::time::Duration;
    use teloxide::types::{InlineKeyboardButtonKind, UserId};
    use super::*;
    use crate::handlers::utils::callbacks::build_callback_query;

    fn config() -> SelfDestructionConfig {
        SelfDestructionConfig {
            notice: Duration::from_secs(120),
            mode: DeletionMode::Enabled,
            ..Default::default()
        }
    }

    fn settings(choices: [(MessageGroup, u32); 1]) -> ChatCleanupSettings {
        ChatCleanupSettings::new(BTreeMap::from(choices.map(|(group, minutes)|
            (group, DelayMinutes::new(minutes)))), None)
    }

    fn round_trip(action: CleanupAction) -> CleanupCallbackData {
        let data = CleanupCallbackData { uid: UserId(12345), action };
        let query = build_callback_query(data.to_data_string());
        CleanupCallbackData::parse(&query).expect("couldn't parse the callback data")
    }

    /// The buttons of every picker already sent carry this exact layout, so it is a contract with
    /// the past, not an implementation detail — a `Display` that renders it differently would
    /// answer those presses with "invalid data".
    #[test]
    fn test_callback_data_wire_format() {
        let data = CleanupCallbackData { uid: UserId(12345), action: CleanupAction::Pick(MessageGroup::Notice) };
        assert_eq!(data.to_data_string(), "cleanup:12345:pick:notice");

        let data = CleanupCallbackData {
            uid: UserId(12345),
            action: CleanupAction::Set(MessageGroup::Report, DelayMinutes::new(15)),
        };
        assert_eq!(data.to_data_string(), "cleanup:12345:set:report:15");

        let data = CleanupCallbackData {
            uid: UserId(12345),
            action: CleanupAction::SetConfirmed(MessageGroup::Event, DelayMinutes::new(5)),
        };
        assert_eq!(data.to_data_string(), "cleanup:12345:setc:event:5");

        let data = CleanupCallbackData { uid: UserId(12345), action: CleanupAction::Follow(MessageGroup::Event) };
        assert_eq!(data.to_data_string(), "cleanup:12345:follow:event");

        let data = CleanupCallbackData { uid: UserId(12345), action: CleanupAction::Inline(true) };
        assert_eq!(data.to_data_string(), "cleanup:12345:inline:on");

        let data = CleanupCallbackData { uid: UserId(12345), action: CleanupAction::Reset };
        assert_eq!(data.to_data_string(), "cleanup:12345:reset");
    }

    /// The payloads a keyboard offers, in order. Language-independent, unlike the labels.
    fn actions_of(keyboard: InlineKeyboardMarkup) -> Vec<String> {
        keyboard.inline_keyboard
            .into_iter()
            .flatten()
            .filter_map(|button| match button.kind {
                InlineKeyboardButtonKind::CallbackData(data) => Some(data),
                _ => None,
            })
            .collect()
    }

    fn english() -> LanguageCode {
        LanguageCode::new("en".to_owned())
    }

    fn viewer(lang_code: &LanguageCode) -> ButtonBuilder<'_> {
        ButtonBuilder { uid: UserId(1), lang_code }
    }

    /// The first level only opens the delay lists; a chat that decided nothing has nothing to take
    /// back, so it isn't offered to.
    #[test]
    fn test_the_first_level_opens_a_list_per_group() {
        let lang = english();
        let screen = overview(&config(), &ChatCleanupSettings::default(), &viewer(&lang));
        assert_eq!(actions_of(screen.keyboard), vec!["cleanup:1:pick:notice", "cleanup:1:pick:report",
                                              "cleanup:1:pick:event", "cleanup:1:pick:application",
                                              "cleanup:1:inline:on"]);
    }

    #[test]
    fn test_a_chat_that_decided_something_can_take_it_back() {
        let lang = english();
        let screen = overview(&config(), &settings([(MessageGroup::Notice, 0)]), &viewer(&lang));
        assert_eq!(actions_of(screen.keyboard), vec!["cleanup:1:pick:notice", "cleanup:1:pick:report",
                                              "cleanup:1:pick:event", "cleanup:1:pick:application",
                                              "cleanup:1:inline:on", "cleanup:1:reset"]);
    }

    /// The switch always offers the opposite of what the chat has now, and a chat that only touched
    /// it still counts as having decided something.
    #[test]
    fn test_the_inline_switch_offers_the_opposite() {
        let lang = english();
        let compressing = ChatCleanupSettings::new(BTreeMap::new(), Some(true));
        let screen = overview(&config(), &compressing, &viewer(&lang));
        assert_eq!(actions_of(screen.keyboard).split_off(4),
                   vec!["cleanup:1:inline:off", "cleanup:1:reset"]);

        // The bot's own list stands in for a chat that said nothing about inline.
        let config = SelfDestructionConfig {
            inline_groups: "notice".parse().expect("couldn't parse the groups"),
            ..config()
        };
        let screen = overview(&config, &ChatCleanupSettings::default(), &viewer(&lang));
        assert_eq!(actions_of(screen.keyboard).split_off(4), vec!["cleanup:1:inline:off"]);
    }

    /// The suggested delays, then the two answers that are always on offer — the second of which is
    /// what keeps a chat from being trapped by a list that no longer holds what it wants.
    #[test]
    fn test_the_second_level_offers_the_delays_and_the_two_ways_out() {
        let config = SelfDestructionConfig {
            delay_options: "5,60".parse().expect("couldn't parse the options"),
            ..config()
        };
        let lang = english();
        let screen = delays(&config, MessageGroup::Report, &viewer(&lang));
        // The text and the keyboard come from one call now, so the screen can be checked whole.
        assert!(screen.text.contains("Leaderboards"), "the prompt names the group: {}", screen.text);
        assert_eq!(actions_of(screen.keyboard), vec!["cleanup:1:set:report:5", "cleanup:1:set:report:60",
                                              "cleanup:1:set:report:0", "cleanup:1:follow:report",
                                              "cleanup:1:back"]);
    }

    #[test]
    fn test_a_bot_that_suggests_nothing_can_still_be_told_to_keep_things() {
        let config = SelfDestructionConfig {
            delay_options: "".parse().expect("couldn't parse the options"),
            ..config()
        };
        let lang = english();
        let screen = delays(&config, MessageGroup::Report, &viewer(&lang));
        assert_eq!(actions_of(screen.keyboard), vec!["cleanup:1:set:report:0", "cleanup:1:follow:report",
                                              "cleanup:1:back"]);
    }

    /// Long lists wrap instead of running off the side of the screen.
    #[test]
    fn test_the_delays_are_laid_out_three_to_a_row() {
        let config = SelfDestructionConfig {
            delay_options: "1,5,15,60,180".parse().expect("couldn't parse the options"),
            ..config()
        };
        let lang = english();
        let screen = delays(&config, MessageGroup::Report, &viewer(&lang));
        let widths = screen.keyboard.inline_keyboard.iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(widths, vec![3, 2, 1, 1, 1]);
    }

    /// Every label the picker asks for exists. A key that doesn't resolve isn't an error anywhere:
    /// rust-i18n hands back the key itself, and it goes onto the button as `…buttons.inline_off`.
    /// Which is what happened when `ButtonKind` spelled its two-word variants without the
    /// underscore the locale files have.
    #[test]
    fn test_every_button_and_group_has_a_label() {
        let lang = english();
        for kind in ButtonKind::iter() {
            let label = button_of(kind, &lang);
            assert!(!label.contains("commands.cleanup"), "{kind} has no label: {label}");
        }
        for group in MessageGroup::iter() {
            let name = group_name(group, &lang);
            assert!(!name.contains("commands.cleanup"), "{group} has no name: {name}");
        }
    }

    /// The label of a button, whatever action it happens to carry.
    fn button_of(kind: ButtonKind, lang_code: &LanguageCode) -> String {
        let keyboard = InlineKeyboardMarkup::new([row![
            viewer(lang_code).of(kind, CleanupAction::Back)]]);
        keyboard.inline_keyboard.concat().remove(0).text
    }

    /// An hour reads as an hour rather than as sixty minutes, and anything that doesn't divide
    /// stays in minutes.
    #[test]
    fn test_a_delay_is_shown_in_the_larger_unit_that_fits() {
        let minutes = |count: u64| format_delay(Duration::from_secs(count * 60), &english());
        assert_eq!(minutes(5), "5 min");
        assert_eq!(minutes(60), "1 h");
        assert_eq!(minutes(180), "3 h");
        assert_eq!(minutes(90), "90 min");
    }

    #[test]
    fn test_callback_data_round_trip() {
        for action in [CleanupAction::Pick(MessageGroup::Notice),
                       CleanupAction::Set(MessageGroup::Application, DelayMinutes::new(60)),
                       CleanupAction::Set(MessageGroup::Notice, DelayMinutes::new(0)),
                       CleanupAction::SetConfirmed(MessageGroup::Report, DelayMinutes::new(5)),
                       CleanupAction::Follow(MessageGroup::Event),
                       CleanupAction::Inline(true),
                       CleanupAction::Inline(false),
                       CleanupAction::Reset,
                       CleanupAction::Back] {
            let parsed = round_trip(action);
            assert_eq!(parsed.uid, UserId(12345));
            assert_eq!(parsed.action, action);
        }
    }
}

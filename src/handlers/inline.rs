use std::str::FromStr;
use rust_i18n::t;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString};
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::*;
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::handlers::{dick, dod, ensure_lang_code, FromRefs, HandlerResult, utils};
use crate::repo::Repositories;

#[derive(Debug, strum_macros::Display, EnumIter, EnumString)]
#[strum(serialize_all = "snake_case")]
enum InlineCommand {
    Grow,
    Top,
    DickOfDay,
}

impl InlineCommand {
    async fn execute(&self, repos: &Repositories, config: AppConfig, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
        match self {
            InlineCommand::Grow => dick::grow_impl(repos, config, from_refs).await,
            InlineCommand::Top => dick::top_impl(repos, config, from_refs).await,
            InlineCommand::DickOfDay => dod::dick_of_day_impl(repos, config, from_refs).await,
        }
    }
}

pub async fn inline_handler(bot: Bot, query: InlineQuery, repos: Repositories) -> HandlerResult {
    let name = utils::get_full_name(&query.from);
    repos.users.create_or_update(query.from.id, name).await?;

    let lang_code = ensure_lang_code(Some(&query.from));
    let btn_label = t!("inline.results.button", locale = &lang_code);
    let results: Vec<InlineQueryResult> = InlineCommand::iter()
        .map(|cmd| cmd.to_string())
        .map(|key| {
            let title = t!(&format!("inline.results.titles.{key}"), locale = &lang_code);
            let content = InputMessageContent::Text(InputMessageContentText::new(
                t!("inline.results.text", locale = &lang_code)));
            let mut article = InlineQueryResultArticle::new(
                key.clone(), title, content
            );
            let buttons = vec![vec![
                InlineKeyboardButton::callback(&btn_label, key)
            ]];
            article.reply_markup.replace(InlineKeyboardMarkup::new(buttons));
            InlineQueryResult::Article(article)
        })
        .collect();

    let mut answer = bot.answer_inline_query(query.id, results);
    answer.cache_time = Some(1);
    answer.await?;
    Ok(())
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              repos: Repositories, config: AppConfig) -> HandlerResult {
    let lang_code = ensure_lang_code(Some(&query.from));
    let chat_id = query.chat_instance.into();
    let from_refs = FromRefs(&query.from, &chat_id);
    let mut answer = bot.answer_callback_query(&query.id);

    if let (Some(inline_msg_id), Some(data)) = (query.inline_message_id, query.data) {
        match InlineCommand::from_str(&data) {
            Ok(cmd) => {
                let text = cmd.execute(&repos, config, from_refs).await?;
                let mut edit = bot.edit_message_text_inline(inline_msg_id, text);
                edit.reply_markup = None;
                edit.parse_mode.replace(Html);
                edit.await?;
            }
            Err(e) => {
                log::error!("unknown callback data: {e}");
                let text = t!("inline.callback.errors.unknown_data", locale = &lang_code);
                answer.text.replace(text);
                answer.show_alert.replace(true);
            }
        }
    } else {
        let text = t!("inline.callback.errors.no_data", locale = &lang_code);
        answer.text.replace(text);
        answer.show_alert.replace(true);
    };

    answer.await?;
    Ok(())
}

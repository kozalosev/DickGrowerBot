mod domain;
mod handlers;
mod repo;
mod help;
mod metrics;
mod config;
mod commands;

use std::env::VarError;
use std::net::SocketAddr;
use futures::future::join_all;
use reqwest::Url;
use rust_i18n::i18n;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::prelude::*;
use teloxide::dptree::deps;
use teloxide::update_listeners::webhooks::{axum_to_router, Options};
use teloxide::update_listeners::UpdateListener;
use crate::handlers::{checks, HelpCommands, LoanCommands, PrivacyCommands, PromoCommandState, StartCommands};
use crate::handlers::{DickCommands, DickOfDayCommands, ImportCommands, PromoCommands};
use crate::handlers::pvp::{BattleCommands, BattleCommandsNoArgs};
use crate::handlers::stats::StatsCommands;
use crate::handlers::utils::locks::LockCallbackServiceFacade;

const ENV_WEBHOOK_URL: &str = "WEBHOOK_URL";

i18n!(fallback = "en");    // load localizations with default parameters

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    dotenvy::dotenv()?;

    pretty_env_logger::init();

    let app_config = config::AppConfig::from_env();
    let database_config = config::DatabaseConfig::from_env()?;
    let db_conn = repo::establish_database_connection(&database_config).await?;

    let handler = dptree::entry()
        .branch(Update::filter_message().filter_command::<StartCommands>().endpoint(handlers::start_cmd_handler))
        .branch(Update::filter_message().filter_command::<HelpCommands>().endpoint(handlers::help_cmd_handler))
        .branch(Update::filter_message().filter_command::<PrivacyCommands>().endpoint(handlers::privacy_cmd_handler))
        .branch(Update::filter_message().filter_command::<DickCommands>().filter(checks::is_group_chat).endpoint(handlers::dick_cmd_handler))
        .branch(Update::filter_message().filter_command::<DickOfDayCommands>().filter(checks::is_group_chat).endpoint(handlers::dod_cmd_handler))
        .branch(Update::filter_message().filter_command::<BattleCommands>().filter(checks::is_group_chat).endpoint(handlers::pvp::cmd_handler))
        .branch(Update::filter_message().filter_command::<BattleCommandsNoArgs>().filter(checks::is_group_chat).endpoint(handlers::pvp::cmd_handler_no_args))
        .branch(Update::filter_message().filter_command::<StatsCommands>().endpoint(handlers::stats::cmd_handler))
        .branch(Update::filter_message().filter_command::<LoanCommands>().filter(checks::is_group_chat).endpoint(handlers::loan::cmd_handler))
        .branch(Update::filter_message().filter_command::<ImportCommands>().filter(checks::is_group_chat).endpoint(handlers::import_cmd_handler))
        .branch(Update::filter_message().filter_command::<PromoCommands>().filter(checks::is_not_group_chat).enter_dialogue::<Message, InMemStorage<PromoCommandState>, PromoCommandState>()
            .branch(dptree::case![PromoCommandState::Start].endpoint(handlers::promo_cmd_handler)))
        .branch(Update::filter_message().enter_dialogue::<Message, InMemStorage<PromoCommandState>, PromoCommandState>()
            .branch(dptree::case![PromoCommandState::Requested].endpoint(handlers::promo_requested_handler)))
        .branch(Update::filter_message().filter(checks::is_not_group_chat).endpoint(checks::handle_not_group_chat))
        .branch(Update::filter_inline_query().filter(checks::inline::is_group_chat).filter(handlers::pvp::inline_filter).endpoint(handlers::pvp::inline_handler))
        .branch(Update::filter_inline_query().filter(handlers::promo_inline_filter).endpoint(handlers::promo_inline_handler))
        .branch(Update::filter_inline_query().filter(checks::inline::is_group_chat).endpoint(handlers::inline_handler))
        .branch(Update::filter_inline_query().filter(checks::inline::is_not_group_chat).endpoint(checks::inline::handle_not_group_chat))
        .branch(Update::filter_chosen_inline_result().filter(handlers::pvp::chosen_inline_result_filter).endpoint(handlers::pvp::inline_chosen_handler))
        .branch(Update::filter_chosen_inline_result().endpoint(handlers::inline_chosen_handler))
        .branch(Update::filter_callback_query().filter(handlers::page_callback_filter).endpoint(handlers::page_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::pvp::callback_filter).endpoint(handlers::pvp::callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::loan::callback_filter).endpoint(handlers::loan::callback_handler))
        .branch(Update::filter_callback_query().endpoint(handlers::callback_handler));

    let bot = Bot::from_env();
    bot.delete_webhook().await?;

    let set_my_commands_requests = _rust_i18n_available_locales()
        .into_iter()
        .map(|locale| commands::set_my_commands(&bot, locale, &app_config.command_toggles));
    let set_my_commands_failed = join_all(set_my_commands_requests)
        .await
        .into_iter()
        .any(|res| res.is_err());
    if set_my_commands_failed {
        Err("couldn't set the bot's commands")?
    }

    let me = bot.get_me().await?;
    let repos = repo::Repositories::new(&db_conn, &app_config);
    let perks = handlers::perks::all(&db_conn, &app_config);
    let incrementor = handlers::utils::Incrementor::from_env(&repos.dicks, perks);
    let help_context = config::build_context_for_help_messages(me, &incrementor, &handlers::ORIGINAL_BOT_USERNAMES)?;
    let help_container = help::render_help_messages(help_context)?;
    let battle_locker = LockCallbackServiceFacade::from_config(app_config.features);

    let webhook_url: Option<Url> = match std::env::var(ENV_WEBHOOK_URL) {
        Ok(env_url) if !env_url.is_empty() => Some(env_url.parse()?),
        Ok(env_url) if env_url.is_empty() => None,
        Err(VarError::NotPresent) => None,
        _ => Err("invalid webhook URL!")?
    };
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let metrics_router = metrics::init();

    let ignore_unknown_updates = |_| Box::pin(async {});
    let deps = deps![
        repos,
        incrementor,
        app_config,
        help_container,
        battle_locker,
        InMemStorage::<PromoCommandState>::new()
    ];

    match webhook_url {
        Some(url) => {
            log::info!("Setting a webhook: {url}");

            let (mut listener, stop_flag, bot_router) = axum_to_router(bot.clone(), Options::new(addr, url)).await?;
            let stop_token = listener.stop_token();

            let error_handler = LoggingErrorHandler::with_custom_text("An error from the update listener");
            let mut dispatcher = Dispatcher::builder(bot, handler)
                .default_handler(ignore_unknown_updates)
                .dependencies(deps)
                .build();
            let bot_fut = dispatcher.dispatch_with_listener(listener, error_handler);

            let srv = tokio::spawn(async move {
                let tcp_listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .map_err(|err| {
                        stop_token.stop();
                        err
                    })?;
                let app = axum::Router::new()
                    .merge(metrics_router)
                    .merge(bot_router);
                axum::serve(tcp_listener, app)
                    .with_graceful_shutdown(stop_flag)
                    .await
            });

            let (res, _) = futures::join!(srv, bot_fut);
            res
        }
        None => {
            log::info!("The polling dispatcher is activating...");

            let bot_fut = tokio::spawn(async move {
                Dispatcher::builder(bot, handler)
                    .default_handler(ignore_unknown_updates)
                    .dependencies(deps)
                    .enable_ctrlc_handler()
                    .build()
                    .dispatch()
                    .await
            });

            let srv = tokio::spawn(async move {
                let tcp_listener = tokio::net::TcpListener::bind(addr).await?;
                axum::serve(tcp_listener, metrics_router)
                    .with_graceful_shutdown(async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("failed to install CTRL+C signal handler");
                        log::info!("Shutdown of the metrics server")
                    })
                    .await
            });

            let (res, _) = futures::join!(srv, bot_fut);
            res
        }
    }?.map_err(Into::into)
}

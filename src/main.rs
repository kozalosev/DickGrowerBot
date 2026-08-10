// `#[tracing::instrument]` and `#[autometrics]` each wrap an async fn's body in another future, and
// the generic handlers in `handlers::language` nest deeply enough that computing their layout
// overflows the default limit of 128 — but only in release, where the layouts are actually built.
#![recursion_limit = "256"]

mod domain;
mod error_handler;
mod handlers;
mod repo;
mod help;
mod metrics;
mod config;
mod commands;
mod users;
mod observability;
mod telegram_observer;
mod scheduler;
mod reload;
mod bans;
mod topics;
mod cache;

#[cfg(test)]
mod test_containers;

use std::net::SocketAddr;
use futures::future::join_all;
use rust_i18n::i18n;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::DpHandlerDescription;
use teloxide::prelude::*;
use teloxide::dptree::{deps, HandlerDescription};
use teloxide::update_listeners::webhooks::{axum_to_router, Options};
use teloxide::update_listeners::{polling_default, UpdateListener};
use cache::Cache;
use config::AppConfig;
use handlers::SupportService;
use handlers::utils::SelfDestructionService;
use crate::handlers::{checks, HandlerDeps, HelpCommands, LanguageCommands, LoanCommands, PrivacyCommands, PromoCommandState, StartCommands, SupportCommandState, SupportCommands};
use crate::handlers::{DickCommands, DickOfDayCommands, ImportCommands, PromoCommands, TopicsCommands};
use crate::handlers::pvp::{BattleCommands, BattleCommandsNoArgs};
use crate::handlers::stats::StatsCommands;
use crate::handlers::utils::locks::LockCallbackServiceFacade;
use crate::error_handler::MetricsErrorHandler;
use crate::repo::Repositories;
use crate::users::LanguageService;

i18n!(fallback = "en");    // load localizations with default parameters

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    dotenvy::dotenv()?;

    let telemetry = observability::init_tracing()?;
    autometrics::prometheus_exporter::init();

    let app_config = AppConfig::from_env();
    let database_config = config::DatabaseConfig::from_env()?;
    let integrations_config = config::IntegrationsConfig::from_env()?;
    let db_conn = repo::establish_database_connection(&database_config).await?;
    let repos = Repositories::new(&db_conn, &app_config);
    let cache = Cache::connect(config::RedisConfig::from_env()).await;
    let language_service = users::init_language_service(&integrations_config, app_config.caches.chat_language,
                                                        repos.chats.clone(), app_config.features.chats_merging).await;
    let ban_list = bans::BanList::load(repos.users.clone()).await;
    let topic_policy = topics::TopicPolicy::new(app_config.caches.chat_topics, repos.chats.clone());

    let handler = dptree::map_with_description(
        DpHandlerDescription::entry(),
        |upd: Update, ls: LanguageService, repos: Repositories, config: AppConfig, self_destruction: SelfDestructionService| {
            let lang_resolver = ls.defer(upd);
            HandlerDeps { repos, config, self_destruction, lang_resolver }
        })
        .branch(Update::filter_message().filter(handlers::setup::migration_filter).endpoint(handlers::setup::migration_handler))
        // /topics goes above the gate it configures, or a chat could lock itself out of its own
        // setting. Being a branch of its own, it needs no exemption inside the gate: this one
        // matches first, and only what falls through reaches the gate below.
        .branch(Update::filter_message().filter_command::<TopicsCommands>().endpoint(handlers::topics::topics_cmd_handler))
        // Above everything a forum chat may have been told to keep out of its other topics, and
        // safe to put there for the same reason the ban gate is where it is: it writes nothing.
        .branch(Update::filter_message().filter(checks::is_group_chat).filter_async(checks::is_forbidden_topic).endpoint(checks::handle_forbidden_topic))
        .branch(Update::filter_message().filter_command::<HelpCommands>().endpoint(handlers::help_cmd_handler))
        .branch(Update::filter_message().filter_command::<PrivacyCommands>().endpoint(handlers::privacy_cmd_handler))
        .branch(Update::filter_message().filter_command::<SupportCommands>().filter(checks::is_not_group_chat).enter_dialogue::<Message, InMemStorage<SupportCommandState>, SupportCommandState>()
            .branch(dptree::case![SupportCommandState::Start].endpoint(handlers::support_cmd_handler)))
        .branch(Update::filter_message().enter_dialogue::<Message, InMemStorage<SupportCommandState>, SupportCommandState>()
            .branch(dptree::case![SupportCommandState::Requested].endpoint(handlers::support_requested_handler)))
        // Everything above the ban gate must not write a single row for the sender: a banned user
        // may still read the policy and reach the owner, but must not come back into the database.
        // /start writes too — it activates a promo code from a deeplink — so it goes below.
        .branch(dptree::filter(checks::is_banned).endpoint(checks::handle_banned))
        .branch(Update::filter_message().filter_command::<StartCommands>().endpoint(handlers::start_cmd_handler))
        .branch(Update::filter_message().filter_command::<LanguageCommands>().endpoint(handlers::language::language_cmd_handler))
        .branch(checks::group_command::<DickCommands>().endpoint(handlers::dick_cmd_handler))
        .branch(checks::group_command::<DickOfDayCommands>().endpoint(handlers::dod_cmd_handler))
        .branch(checks::group_command::<BattleCommands>().endpoint(handlers::pvp::pvp_cmd_handler))
        .branch(checks::group_command::<BattleCommandsNoArgs>().endpoint(handlers::pvp::pvp_cmd_handler_no_args))
        .branch(checks::group_command::<LoanCommands>().endpoint(handlers::loan::loan_cmd_handler))
        .branch(checks::group_command::<ImportCommands>().endpoint(handlers::import_cmd_handler))
        .branch(Update::filter_message().filter_command::<StatsCommands>().branch(checks::require_anchored_group()).endpoint(handlers::stats::stats_cmd_handler))
        .branch(Update::filter_message().filter_command::<PromoCommands>().filter(checks::is_not_group_chat).enter_dialogue::<Message, InMemStorage<PromoCommandState>, PromoCommandState>()
            .branch(dptree::case![PromoCommandState::Start].endpoint(handlers::promo_cmd_handler)))
        .branch(Update::filter_message().enter_dialogue::<Message, InMemStorage<PromoCommandState>, PromoCommandState>()
            .branch(dptree::case![PromoCommandState::Requested].endpoint(handlers::promo_requested_handler)))
        .branch(Update::filter_message().filter(checks::is_not_group_chat).endpoint(checks::handle_not_group_chat))
        .branch(Update::filter_inline_query().filter(checks::inline::is_group_chat).filter(handlers::pvp::inline_filter).endpoint(handlers::pvp::pvp_inline_handler))
        .branch(Update::filter_inline_query().filter(handlers::promo_inline_filter).endpoint(handlers::promo_inline_handler))
        .branch(Update::filter_inline_query().filter(checks::inline::is_group_chat).endpoint(handlers::inline_handler))
        .branch(Update::filter_inline_query().filter(checks::inline::is_not_group_chat).endpoint(checks::inline::handle_not_group_chat_inline))
        .branch(Update::filter_chosen_inline_result().filter(handlers::pvp::chosen_inline_result_filter).endpoint(handlers::pvp::pvp_inline_chosen_handler))
        .branch(Update::filter_chosen_inline_result().endpoint(handlers::inline_chosen_handler))
        // The rights are recorded before the branch, not by one, because the setup message consumes
        // the very update that adds the bot to a group — as an administrator too, in one step.
        .branch(Update::filter_my_chat_member()
            .inspect_async(handlers::rights::remember_bot_rights)
            .filter(handlers::setup::added_to_legacy_group_filter)
            .endpoint(handlers::setup::added_to_legacy_group_handler))
        // The buttons need the same gate as the commands: a keyboard outlives the message it came
        // with, and the restriction may well be younger than both.
        .branch(Update::filter_callback_query().filter_async(checks::is_forbidden_topic_callback).endpoint(checks::handle_forbidden_topic_callback))
        .branch(Update::filter_callback_query().filter(handlers::setup::callback_filter).endpoint(handlers::setup::setup_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::page_callback_filter).endpoint(handlers::page_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::shrink::callback_filter).endpoint(handlers::shrink::shrink_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::pvp::callback_filter).endpoint(handlers::pvp::pvp_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::loan::callback_filter).endpoint(handlers::loan::loan_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::language::callback_filter).endpoint(handlers::language::language_callback_handler))
        .branch(Update::filter_callback_query().filter(handlers::topics::callback_filter).endpoint(handlers::topics::topics_callback_handler))
        .branch(Update::filter_callback_query().endpoint(handlers::inline_callback_handler));

    let bot = config::BotConfig::build_bot()?;
    bot.delete_webhook().await?;

    // The personal /language relies on the user-service; when it's unavailable, hide /language from
    // the private-chat menu. The chat-wide /language (groups, admins-only) stays available regardless.
    let command_toggles = commands::CommandToggles {
        env: app_config.command_toggles.clone(),
        personal_language_enabled: language_service.user_service_enabled(),
        support_enabled: app_config.support_chat_id.is_some(),
    };
    let locales = _rust_i18n_available_locales();
    let set_my_commands_requests = locales
        .iter()
        .map(|locale| commands::set_my_commands(&bot, locale, &command_toggles));
    join_all(set_my_commands_requests)
        .await
        .into_iter()
        .collect::<Result<(), _>>()
        .map_err(|err| format!("couldn't set the bot's commands: {err}"))?;

    let me = bot.get_me().await?;
    let perks = handlers::perks::all(&db_conn, &app_config);
    let incrementor = handlers::utils::Incrementor::new(app_config.incrementor.clone(), &repos.dicks, perks);
    let help_context = config::build_context_for_help_messages(&me, &incrementor, &handlers::ORIGINAL_BOT_USERNAMES)?;
    let help_container = help::render_help_messages(help_context)?;
    let battle_locker = LockCallbackServiceFacade::from_config(app_config.features);
    let self_destruction = SelfDestructionService::new(app_config.self_destruction,
                                                       repos.deletions.clone(), cache.clone(),
                                                       me.user.id);
    let support_service = SupportService::new(app_config.support_chat_id);

    let webhook_url = integrations_config.webhook_url;
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let (metrics_router, prometheus_layer) = metrics::init();
    metrics::register_db_pool_collector(db_conn.clone());

    // Best-effort background job that shrinks inactive dicks at each UTC midnight. Spawned before
    // `deps!` moves the shared services, and before the webhook/polling split so it runs in both.
    // One throttle for both schedulers: it counts the requests in a worker of its own, so a second
    // one would count a second budget and let twice as much through.
    // TODO: [#153] Use a common `Throttle` object shared between handlers and schedulers
    let throttled_bot = scheduler::throttled(bot.clone(), config::ThrottleConfig::from_env());
    scheduler::spawn_daily_shrink(throttled_bot.clone(), repos.clone(), language_service.clone(),
                                  topic_policy.clone(), app_config.clone());
    scheduler::spawn_deletion_worker(throttled_bot, repos.clone(), cache.clone(), app_config.clone());
    scheduler::spawn_deletion_cleaner(repos.clone(), app_config.clone());
    reload::spawn_reload_on_sighup(repos.announcements.clone(), ban_list.clone());
    ban_list.spawn_refresh_task(app_config.caches.ban_list_refresh);

    let ignore_unknown_updates = |_| Box::pin(async {});
    let deps = deps![
        me,
        repos,
        incrementor,
        app_config,
        help_container,
        battle_locker,
        language_service,
        self_destruction,
        support_service,
        ban_list,
        topic_policy,
        cache,
        InMemStorage::<PromoCommandState>::new(),
        InMemStorage::<SupportCommandState>::new()
    ];

    let join_result = match webhook_url {
        Some(url) => {
            tracing::info!(url = %url, "setting a webhook");

            let (mut listener, stop_flag, bot_router) = axum_to_router(bot.clone(), Options::new(addr, url)).await?;
            let stop_token = listener.stop_token();

            let error_handler = LoggingErrorHandler::with_custom_text("An error from the update listener");
            let mut dispatcher = Dispatcher::builder(bot, handler)
                .default_handler(ignore_unknown_updates)
                .error_handler(MetricsErrorHandler::new("An error in a handler"))
                .dependencies(deps)
                .build();
            let bot_fut = dispatcher.dispatch_with_listener(listener, error_handler);

            let srv = tokio::spawn(metrics::TASK_WEBHOOK_SERVER.instrument(async move {
                let tcp_listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .inspect_err(|_| stop_token.stop())?;
                let app = axum::Router::new()
                    .merge(metrics_router)
                    .merge(bot_router)
                    .layer(prometheus_layer);
                axum::serve(tcp_listener, app)
                    .with_graceful_shutdown(stop_flag)
                    .await
            }));

            let (res, _) = futures::join!(srv, bot_fut);
            res
        }
        None => {
            tracing::info!("the polling dispatcher is activating");

            let bot_fut = tokio::spawn(metrics::TASK_POLLING_DISPATCHER.instrument(async move {
                let listener = polling_default(bot.clone()).await;
                let listener_error_handler = MetricsErrorHandler::new("An error from the update listener");
                Dispatcher::builder(bot, handler)
                    .default_handler(ignore_unknown_updates)
                    .error_handler(MetricsErrorHandler::new("An error in a handler"))
                    .dependencies(deps)
                    .enable_ctrlc_handler()
                    .build()
                    .dispatch_with_listener(listener, listener_error_handler)
                    .await
            }));

            let srv = tokio::spawn(metrics::TASK_METRICS_SERVER.instrument(async move {
                let tcp_listener = tokio::net::TcpListener::bind(addr).await?;
                axum::serve(tcp_listener, metrics_router.layer(prometheus_layer))
                    .with_graceful_shutdown(async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("failed to install CTRL+C signal handler");
                        tracing::info!("shutting the metrics server down")
                    })
                    .await
            }));

            let (res, _) = futures::join!(srv, bot_fut);
            res
        }
    };

    telemetry.shutdown()?;
    join_result?.map_err(Into::into)
}

mod handlers;
mod repo;
mod help;
mod metrics;
mod config;

use std::env::VarError;
use std::net::SocketAddr;
use axum::Router;
use reqwest::Url;
use rust_i18n::i18n;
use teloxide::prelude::*;
use dotenvy::dotenv;
use refinery::config::Config;
use teloxide::dptree::deps;
use teloxide::update_listeners::webhooks::{axum_to_router, Options};
use crate::handlers::{DickCommands, DickOfDayCommands, HelpCommands};


const ENV_WEBHOOK_URL: &str = "WEBHOOK_URL";

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!();
}

i18n!();    // load localizations with default parameters

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv()?;
    pretty_env_logger::init();

    let app_config = config::AppConfig::from_env();
    let database_config = config::DatabaseConfig::from_env()?;

    let handler = dptree::entry()
        .branch(Update::filter_message().filter_command::<HelpCommands>().endpoint(handlers::help_cmd_handler))
        .branch(Update::filter_message().filter_command::<DickCommands>().endpoint(handlers::dick_cmd_handler))
        .branch(Update::filter_message().filter_command::<DickOfDayCommands>().endpoint(handlers::dod_cmd_handler));
        // TODO: inline mode
        //.branch(Update::filter_inline_query().endpoint(handlers::inline_handler))
        //.branch(Update::filter_chosen_inline_result().endpoint(handlers::inline_chosen_handler))
        //.branch(Update::filter_callback_query().endpoint(handlers::callback_handler));

    let bot = Bot::from_env();
    bot.delete_webhook().await?;

    let webhook_url: Option<Url> = match std::env::var(ENV_WEBHOOK_URL) {
        Ok(env_url) if env_url.len() > 0 => Some(env_url.parse()?),
        Ok(env_url) if env_url.len() == 0 => None,
        Err(VarError::NotPresent) => None,
        _ => Err("invalid webhook URL!")?
    };
    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let metrics_router = metrics::init();

    run_migrations(&database_config).await?;
    let db_conn = establish_database_connection(&database_config).await?;
    let deps = deps![
        repo::Users::new(db_conn.clone()),
        repo::Dicks::new(db_conn.clone()),
        repo::Imports::new(db_conn.clone()),
        app_config
    ];

    match webhook_url {
        Some(url) => {
            log::info!("Setting a webhook: {url}");

            let (listener, stop_flag, bot_router) = axum_to_router(bot.clone(), Options::new(addr, url)).await?;

            let error_handler = LoggingErrorHandler::with_custom_text("An error from the update listener");
            let mut dispatcher = Dispatcher::builder(bot, handler)
                .dependencies(deps)
                .build();
            let bot_fut = dispatcher.dispatch_with_listener(listener, error_handler);

            let srv = tokio::spawn(async move {
                axum::Server::bind(&addr)
                    .serve(Router::new()
                        .merge(metrics_router)
                        .merge(bot_router)
                        .into_make_service())
                    .with_graceful_shutdown(stop_flag)
                    .await
            }
            );

            let (res, _) = futures::join!(srv, bot_fut);
            res?.map_err(|e| e.into()).into()
        }
        None => {
            log::info!("The polling dispatcher is activating...");

            let bot_fut = tokio::spawn(async move {
                Dispatcher::builder(bot, handler)
                    .dependencies(deps)
                    .enable_ctrlc_handler()
                    .build()
                    .dispatch()
                    .await
            });

            let srv = tokio::spawn(async move {
                axum::Server::bind(&addr)
                    .serve(metrics_router.into_make_service())
                    .with_graceful_shutdown(async {
                        tokio::signal::ctrl_c()
                            .await
                            .expect("failed to install CTRL+C signal handler");
                        log::info!("Shutdown of the metrics server")
                    })
                    .await
            });

            let (res, _) = futures::join!(srv, bot_fut);
            res?.map_err(|e| e.into()).into()
        }
    }
}

async fn establish_database_connection(config: &config::DatabaseConfig) -> Result<sqlx::Pool<sqlx::Postgres>, anyhow::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(config.url.as_str()).await
        .map_err(|e| e.into())
}

async fn run_migrations(config: &config::DatabaseConfig) -> anyhow::Result<()> {
    let mut conn = Config::try_from(config.url.clone())?;
    embedded::migrations::runner().run_async(&mut conn).await?;
    Ok(())
}

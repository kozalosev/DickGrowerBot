//! Stopping the bot on purpose, so that what it was in the middle of survives the stop.
//!
//! The sibling of [`crate::reload`]: both are about signals, and between them they say what each
//! one means. SIGHUP reloads, SIGTERM and SIGINT stop; a signal cannot mean both.

use teloxide::dispatching::ShutdownToken;

/// Stops the dispatcher on the first stop signal.
///
/// Stopping the dispatcher is the whole lever: it stops its update listener too, which is what the
/// webhook server's graceful shutdown is waiting on. So one token drains everything in order —
/// no new updates, then the ones in flight, then the HTTP server, then `Telemetry::shutdown` back
/// in `main` flushes the last spans and log records.
pub fn spawn_stop_on_signal(token: ShutdownToken) {
    tokio::spawn(async move {
        stop_signal().await;
        tracing::info!("a stop signal arrived, shutting the bot down");
        match token.shutdown() {
            Ok(finished) => finished.await,
            Err(e) => tracing::warn!(error = %e, "the dispatcher was not running when the signal arrived"),
        }
    });
}

/// Resolves when the process is asked to stop.
///
/// **SIGTERM is the one that matters**: it is what `docker stop` sends, so it arrives on every
/// deploy. SIGINT is Ctrl-C, which is how the bot is stopped while being worked on.
///
/// SIGHUP is deliberately absent: it is [`crate::reload`]'s, and a signal that meant both would
/// leave the bot reloading when it was asked to stop — after which Docker waits out its timeout and
/// kills it, losing everything that only happens on a clean exit.
///
/// Awaiting this from two places at once is fine: every listener of a signal receives it.
#[cfg(unix)]
pub async fn stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let Ok(mut terminate) = signal(SignalKind::terminate())
        .inspect_err(|e| tracing::error!(error = %e, "couldn't listen for SIGTERM, the bot will only stop on Ctrl-C"))
    else {
        return ctrl_c().await
    };
    tokio::select! {
        _ = terminate.recv() => {},
        () = ctrl_c() => {},
    }
}

/// Windows has no SIGTERM, and the bot only runs there for development.
#[cfg(not(unix))]
pub async fn stop_signal() {
    ctrl_c().await
}

async fn ctrl_c() {
    tokio::signal::ctrl_c().await
        .unwrap_or_else(|e| tracing::error!(error = %e, "couldn't listen for Ctrl-C"));
}

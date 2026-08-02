//! Reloading parts of the configuration while the bot runs, without a restart (issue #137).
//!
//! Deliberately above both `config` and `repo`: the values are read by `config`, but the state they
//! replace is held by `repo`, and `repo` already depends on `config`.

use crate::repo::Announcements;

/// Reloads the announcements file on every SIGHUP, so a new announcement can be published with
/// `docker kill -s HUP dickgrowerbot` instead of a restart.
///
/// Only the announcements. Every other value is read from the environment once at startup and then
/// baked into the handlers, the repositories, the help messages and the incrementor.
#[cfg(unix)]
pub fn spawn_reload_on_sighup(announcements: Announcements) {
    use tokio::signal::unix::{signal, SignalKind};
    use crate::config::get_env_value_or_default;

    let path = get_env_value_or_default("ANNOUNCEMENTS_FILE", "announcements.yml".to_owned());
    tokio::spawn(async move {
        let mut hangups = match signal(SignalKind::hangup()) {
            Ok(hangups) => hangups,
            Err(e) => {
                tracing::error!(error = %e, "couldn't listen for SIGHUP, the announcements will only change on a restart");
                return
            }
        };
        tracing::info!(path = %path, "send SIGHUP to reload the announcements");
        while hangups.recv().await.is_some() {
            announcements.reload(&path);
        }
    });
}

/// Windows has no SIGHUP, and the bot only runs there for development.
#[cfg(not(unix))]
pub fn spawn_reload_on_sighup(_announcements: Announcements) {
    tracing::info!("this platform has no SIGHUP, the announcements will only change on a restart");
}

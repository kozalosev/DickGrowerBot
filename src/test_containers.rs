//! Containers the test binary shares, and the runtime that owns them.
//!
//! A `#[tokio::test]` builds a runtime of its own and tears it down when the test ends, and both a
//! sqlx pool and a redis `ConnectionManager` die with the runtime that created them. So everything
//! shared lives on the runtime here instead, and tests reach it with [`spawn`] — `block_on` would
//! panic, being called from inside a runtime already.
//!
//! The containers deliberately outlive the run: each is marked reusable, so the next run finds it
//! instead of paying the startup again. `task test:clean` is the only thing that removes them.

use once_cell::sync::Lazy;
use testcontainers::{ContainerAsync, GenericImage, ImageExt, ReuseDirective};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::OnceCell;

/// Put on every container the tests share; `task test:clean` finds them by this key, which is why
/// it matches by key alone and sweeps whatever the value is.
pub const LABEL: &str = "dickgrowerbot.test";

/// A container started once for the whole test binary.
///
/// Declared as a `static`, which is what the `const` constructor is for. Whether a request is
/// answered by an existing container is decided by the **labels**, so [`SharedContainer::service`]
/// has to differ between them: labelled alike, the second request is handed the first container
/// and fails on a port that isn't there.
pub struct SharedContainer {
    service: &'static str,
    image: &'static str,
    tag: &'static str,
    port: u16,
    /// Waited for on stdout, once per entry — a server that prints its line while setting up and
    /// again when it is really up needs that line listed twice.
    ready: &'static [&'static str],
    /// Waited out after those lines, for a server whose port opens a moment behind them.
    settle_millis: u64,
    env: &'static [(&'static str, &'static str)],
    started: OnceCell<(ContainerAsync<GenericImage>, u16)>,
}

impl SharedContainer {
    pub const fn new(
        service: &'static str,
        image: &'static str,
        tag: &'static str,
        port: u16,
        ready: &'static [&'static str],
    ) -> Self {
        Self {
            service, image, tag, port, ready,
            settle_millis: 0,
            env: &[],
            started: OnceCell::const_new(),
        }
    }

    pub const fn with_settle_millis(mut self, millis: u64) -> Self {
        self.settle_millis = millis;
        self
    }

    pub const fn with_env(mut self, env: &'static [(&'static str, &'static str)]) -> Self {
        self.env = env;
        self
    }

    /// The port on the host, starting the container the first time it is asked for.
    pub async fn port(&'static self) -> u16 {
        spawn(async { self.started.get_or_init(|| self.start()).await.1 })
            .await
            .unwrap_or_else(|e| panic!("the shared {} container task failed: {e}", self.service))
    }

    async fn start(&'static self) -> (ContainerAsync<GenericImage>, u16) {
        let mut image = GenericImage::new(self.image, self.tag)
            .with_exposed_port(self.port.tcp());
        for line in self.ready {
            image = image.with_wait_for(WaitFor::message_on_stdout(*line));
        }
        if self.settle_millis > 0 {
            image = image.with_wait_for(WaitFor::millis(self.settle_millis));
        }

        let mut image = image
            .with_label(LABEL, self.service)
            .with_reuse(ReuseDirective::Always);
        for (key, value) in self.env {
            image = image.with_env_var(*key, *value);
        }

        let container = image.start()
            .await
            .unwrap_or_else(|e| panic!("couldn't start the {} container: {e}", self.service));
        let port = container.get_host_port_ipv4(self.port)
            .await
            .unwrap_or_else(|e| panic!("couldn't fetch the port of the {} container: {e}", self.service));
        (container, port)
    }
}

/// Runs something on the shared runtime, for whatever a test needs to outlive itself.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME.spawn(future)
}

static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
    Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("couldn't build the shared test runtime")
});

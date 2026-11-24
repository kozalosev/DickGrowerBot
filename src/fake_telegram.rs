//! A Bot API served from this process, for tests that need to see what the bot asks Telegram.

use std::sync::{Arc, Mutex};
use axum::{Json, Router};
use serde_json::Value;
use teloxide::Bot;
use crate::config::{ThrottleConfig, ThrottledBot};

/// The path and the body of each request, in the order they arrived.
pub type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// A running fake API. Dropping it stops the server.
pub struct FakeApi {
    pub bot: ThrottledBot,
    calls: Calls,
    server: tokio::task::JoinHandle<()>,
}

impl FakeApi {
    /// Serves every request with what `answer` returns for its method, such as `"DeleteMessage"`.
    pub async fn start(answer: impl Fn(&str) -> Value + Clone + Send + Sync + 'static) -> Self {
        let calls = Calls::default();
        let recorded = Arc::clone(&calls);
        let app = Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = Arc::clone(&recorded);
            let answer = answer.clone();
            async move {
                let path = request.uri().path().to_owned();
                let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await.expect("couldn't read the request body");
                let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let method = path.rsplit('/').next().unwrap_or_default().to_owned();
                recorded.lock().expect("the calls are poisoned").push((path, body));
                Json(answer(&method))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await.expect("couldn't bind the fake API");
        let address = listener.local_addr().expect("the fake API has no address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("the fake API stopped");
        });
        let api_url = reqwest::Url::parse(&format!("http://{address}/")).expect("invalid API URL");
        let bot = ThrottleConfig::from_env().wrap(Bot::new("123:token").set_api_url(api_url));
        Self { bot, calls, server }
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("the calls are poisoned").clone()
    }

    /// The methods that were called, in order.
    pub fn methods(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .map(|(path, _)| path.rsplit('/').next().unwrap_or_default().to_owned())
            .collect()
    }
}

impl Drop for FakeApi {
    fn drop(&mut self) {
        self.server.abort()
    }
}

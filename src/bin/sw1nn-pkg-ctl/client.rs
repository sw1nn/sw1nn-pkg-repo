//! HTTP client that attaches the stored access token and renews it when needed.

use crate::authelia::{self, Provider};
use crate::token_store::{self, Tokens};
use colored::Colorize;
use std::process;
use tokio::sync::Mutex;

/// Refresh this many seconds before the access token actually expires, so a
/// request in flight does not lapse mid-way.
const EXPIRY_SKEW_SECS: i64 = 60;

/// A `reqwest::Client` that keeps the access token fresh.
///
/// Renewal happens in two places: proactively when the stored token is about to
/// expire, and reactively on a single `401`. There is deliberately no background
/// refresh daemon — the CLI runs synchronously in a terminal with a human
/// present, so an expired session is reported rather than worked around.
pub struct ApiClient {
    http: reqwest::Client,
    provider: Provider,
    tokens: Mutex<Option<Tokens>>,
}

impl ApiClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().build()?,
            provider: Provider::default(),
            tokens: Mutex::new(token_store::load()),
        })
    }

    /// Start a request. The caller adds body/headers, then hands the builder to
    /// [`ApiClient::send`].
    pub fn post<U>(&self, url: U) -> reqwest::RequestBuilder
    where
        U: reqwest::IntoUrl,
    {
        self.http.post(url)
    }

    pub fn get<U>(&self, url: U) -> reqwest::RequestBuilder
    where
        U: reqwest::IntoUrl,
    {
        self.http.get(url)
    }

    /// Send a request with the current access token, renewing once on `401`.
    ///
    /// Requests with an unclonable (streaming) body cannot be replayed; every
    /// call site in this CLI builds its body in memory, so cloning succeeds.
    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let replay = request.try_clone();

        let token = self.access_token().await;
        let response = self.dispatch(request, token.as_deref()).await?;

        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }

        // The server rejected the token even though we thought it was current
        // — the clock may be off, or the token was revoked. Renew once.
        let Some(replay) = replay else {
            return Ok(response);
        };

        let renewed = self.renew().await;
        self.dispatch(replay, Some(&renewed)).await
    }

    async fn dispatch(
        &self,
        request: reqwest::RequestBuilder,
        token: Option<&str>,
    ) -> Result<reqwest::Response, reqwest::Error> {
        match token {
            Some(token) => request.bearer_auth(token).send().await,
            None => request.send().await,
        }
    }

    /// The access token to use now, refreshing first if it is about to expire.
    ///
    /// Returns `None` when nothing is stored, so requests to a server running
    /// without authentication still work.
    async fn access_token(&self) -> Option<String> {
        let stale = {
            let tokens = self.tokens.lock().await;
            match tokens.as_ref() {
                None => return None,
                Some(t) => t.is_stale(EXPIRY_SKEW_SECS),
            }
        };

        if stale {
            return Some(self.renew().await);
        }

        self.tokens
            .lock()
            .await
            .as_ref()
            .map(|t| t.access_token.clone())
    }

    /// Exchange the refresh token for a new access token and persist the result.
    ///
    /// Never returns when renewal is impossible: in a git hook or a script this
    /// is the point at which a human needs to act.
    async fn renew(&self) -> String {
        let mut tokens = self.tokens.lock().await;

        let Some(refresh_token) = tokens.as_ref().and_then(|t| t.refresh_token.clone()) else {
            session_expired()
        };

        match authelia::refresh(&self.http, &self.provider, &refresh_token).await {
            Ok(renewed) => {
                if let Err(e) = token_store::save(&renewed) {
                    tracing::warn!(error = %e, "Refreshed token could not be saved");
                }
                let access_token = renewed.access_token.clone();
                *tokens = Some(renewed);
                access_token
            }
            Err(e) => {
                tracing::debug!(error = %e, "Token refresh failed");
                session_expired()
            }
        }
    }
}

/// Report an unrecoverable session failure and exit.
fn session_expired() -> ! {
    eprintln!("{}: not logged in or session expired", "error".red().bold());
    eprintln!("       run: {}", "sw1nn-pkg-ctl login".cyan());
    process::exit(1);
}

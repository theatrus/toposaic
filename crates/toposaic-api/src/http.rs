use std::time::Duration;

use anyhow::{Context, Result};
use axum::http::{HeaderValue, Uri};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

pub(crate) const USER_AGENT: &str = concat!(
    "toposaic/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/theatrus/toposaic)"
);

pub(crate) fn async_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
        .context("build HTTP client")
}

pub(crate) fn blocking_client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
        .context("build HTTP client")
}

pub(crate) fn cors_layer(configured_origins: Vec<String>) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin, _| {
            trusted_origin(origin, &configured_origins)
        }))
        .allow_methods(Any)
        .allow_headers(Any)
}

fn trusted_origin(origin: &HeaderValue, configured_origins: &[String]) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    if matches!(
        origin,
        "tauri://localhost"
            | "http://tauri.localhost"
            | "https://toposaic.com"
            | "https://www.toposaic.com"
    ) || configured_origins.iter().any(|allowed| allowed == origin)
    {
        return true;
    }
    let Ok(uri) = origin.parse::<Uri>() else {
        return false;
    };
    uri.scheme_str() == Some("http")
        && matches!(
            uri.host(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_tracks_the_crate_version_and_current_repository() {
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
        assert!(USER_AGENT.contains("github.com/theatrus/toposaic"));
        assert!(!USER_AGENT.contains("terrain-puzzle"));
    }

    #[test]
    fn cors_only_accepts_the_app_site_loopback_and_configured_origins() {
        let configured = vec!["https://studio.example.com".to_owned()];
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "http://127.0.0.1:1420",
            "http://localhost:3100",
            "http://[::1]:8787",
            "https://toposaic.com",
            "https://www.toposaic.com",
            "https://studio.example.com",
        ] {
            assert!(
                trusted_origin(&HeaderValue::from_str(origin).unwrap(), &configured),
                "{origin}"
            );
        }
        for origin in [
            "https://evil.example",
            "https://toposaic.com.evil.example",
            "http://192.0.2.10:3100",
        ] {
            assert!(
                !trusted_origin(&HeaderValue::from_str(origin).unwrap(), &configured),
                "{origin}"
            );
        }
    }
}

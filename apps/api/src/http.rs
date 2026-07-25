use std::time::Duration;

use anyhow::{Context, Result};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_tracks_the_crate_version_and_current_repository() {
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
        assert!(USER_AGENT.contains("github.com/theatrus/toposaic"));
        assert!(!USER_AGENT.contains("terrain-puzzle"));
    }
}

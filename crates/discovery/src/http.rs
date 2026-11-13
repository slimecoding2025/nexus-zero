//! Real HTTPS transport for the GitHub adapter (cargo feature `http`).
//!
//! **Status: written but never compiled or run.** It was written in an
//! environment with no access to crates.io, so `ureq` could not be fetched.
//! It targets the `ureq` 2.x API from memory of its documentation. Build it
//! with `cargo build -p nexus-discovery --features http`; expect to fix
//! small API mismatches. It has never talked to the real GitHub API.
//!
//! ## What it guarantees (by construction, not by test)
//!
//! - The token is added only to URLs that start with the configured origin
//!   (default [`PUBLIC_API_BASE`]); any other URL is refused before a
//!   request is made.
//! - Redirects are never followed (`redirects(0)`), so the token cannot be
//!   sent to another host.
//! - A non-2xx status is returned as a normal [`Response`]; only network
//!   failures are a [`TransportError`].
//! - Bodies are read up to a limit plus one byte, so an oversized body is
//!   detected by the client without being loaded in full.
//! - Only `https://` origins are accepted.

use std::io::Read;
use std::time::Duration;

use crate::github::{
    Headers, Response, Token, Transport, TransportError, DEFAULT_MAX_BODY_BYTES, PUBLIC_API_BASE,
};

const USER_AGENT: &str = concat!("nexus-zero-discovery/", env!("CARGO_PKG_VERSION"));
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The origin was not a usable `https://` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOrigin;

impl std::fmt::Display for InvalidOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("origin must start with https:// and have no trailing slash")
    }
}

impl std::error::Error for InvalidOrigin {}

/// A [`Transport`] backed by `ureq` (rustls).
pub struct UreqTransport {
    agent: ureq::Agent,
    token: Token,
    origin: String,
    max_body_bytes: usize,
}

impl UreqTransport {
    /// Transport for the public GitHub API.
    pub fn new(token: Token) -> Self {
        Self::with_origin(token, PUBLIC_API_BASE).expect("PUBLIC_API_BASE is a valid origin")
    }

    /// Transport for another origin, for example GitHub Enterprise Server
    /// (`https://ghe.example.com/api/v3`).
    pub fn with_origin(token: Token, origin: &str) -> Result<Self, InvalidOrigin> {
        if !origin.starts_with("https://") || origin.len() <= "https://".len() || origin.ends_with('/')
        {
            return Err(InvalidOrigin);
        }
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout(DEFAULT_TIMEOUT)
            .user_agent(USER_AGENT)
            .build();
        Ok(Self {
            agent,
            token,
            origin: origin.to_string(),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        })
    }

    /// Cap on the bytes read from one body. Set it to the same value as
    /// `SyncOptions::max_body_bytes` so the client reports `BodyTooLarge`.
    pub fn with_max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = max;
        self
    }

    fn url_allowed(&self, url: &str) -> bool {
        url == self.origin
            || url
                .strip_prefix(self.origin.as_str())
                .map_or(false, |rest| rest.starts_with('/') || rest.starts_with('?'))
    }
}

impl std::fmt::Debug for UreqTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UreqTransport")
            .field("origin", &self.origin)
            .field("token", &self.token)
            .finish()
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError> {
        if !self.url_allowed(url) {
            return Err(TransportError(
                "refused: URL is outside the configured origin".to_string(),
            ));
        }
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        request = request.set(
            "Authorization",
            &format!("Bearer {}", self.token.expose()),
        );

        let response = match request.call() {
            Ok(r) => r,
            // ureq reports 4xx/5xx (and 3xx when redirects are off) as errors.
            Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                // `Display` of ureq's transport error does not include
                // request headers, so the token cannot appear here.
                return Err(TransportError(t.to_string()));
            }
        };
        read_response(response, self.max_body_bytes)
    }
}

fn read_response(response: ureq::Response, max_body_bytes: usize) -> Result<Response, TransportError> {
    let status = response.status();
    let mut headers = Headers::new();
    for name in response.headers_names() {
        // Repeated headers are joined with commas, as the trait requires.
        let joined = response.all(&name).join(", ");
        headers.insert(&name, &joined);
    }

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(max_body_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| TransportError(format!("reading body failed: {e}")))?;
    let body = String::from_utf8(bytes)
        .map_err(|_| TransportError("response body is not valid UTF-8".to_string()))?;

    Ok(Response {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(origin: &str) -> UreqTransport {
        UreqTransport::with_origin(Token::new("t").unwrap(), origin).unwrap()
    }

    #[test]
    fn rejects_non_https_and_trailing_slash_origins() {
        let t = || Token::new("t").unwrap();
        assert!(UreqTransport::with_origin(t(), "http://api.github.com").is_err());
        assert!(UreqTransport::with_origin(t(), "https://").is_err());
        assert!(UreqTransport::with_origin(t(), "https://api.github.com/").is_err());
        assert!(UreqTransport::with_origin(t(), "https://api.github.com").is_ok());
    }

    #[test]
    fn only_urls_under_the_origin_are_allowed() {
        let t = transport("https://api.github.com");
        assert!(t.url_allowed("https://api.github.com/orgs/acme/repos?page=2"));
        assert!(t.url_allowed("https://api.github.com?x=1"));
        assert!(!t.url_allowed("https://api.github.com.evil.example/orgs"));
        assert!(!t.url_allowed("https://evil.example/https://api.github.com/"));
        assert!(!t.url_allowed("http://api.github.com/orgs"));
    }

    #[test]
    fn debug_output_does_not_contain_the_token() {
        let t = UreqTransport::with_origin(Token::new("ghp_secret").unwrap(), PUBLIC_API_BASE)
            .unwrap();
        assert!(!format!("{t:?}").contains("ghp_secret"));
    }

    #[test]
    fn refuses_foreign_url_without_network() {
        let t = transport("https://api.github.com");
        let err = t.get("https://evil.example/x", &[]).unwrap_err();
        assert!(err.0.contains("outside the configured origin"));
    }
}

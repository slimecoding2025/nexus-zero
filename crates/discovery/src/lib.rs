//! Provider discovery adapters for NEXUS ZERO.
//!
//! Current state, stated plainly:
//! - One adapter exists: [`github`]. It builds a repository inventory
//!   (one `Repository` node per repository) for one organization or user.
//! - It discovers no relationships (edges) yet.
//! - The network is behind the [`github::Transport`] trait. The only
//!   implementation that is built and tested is a test double. An HTTPS
//!   transport exists behind the optional `http` feature, but it was
//!   written without being compiled and has never run. Nothing here has
//!   talked to the real GitHub API.
//!
//! Adapters never sleep, retry or write anywhere. They report what they
//! saw, including partial results and how long a rate limit asks the caller
//! to wait, and the caller decides what to do next.

pub mod github;
#[cfg(feature = "http")]
pub mod http;
mod json;

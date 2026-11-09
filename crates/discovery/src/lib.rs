//! Provider discovery adapters for NEXUS ZERO.
//!
//! Current state, stated plainly:
//! - One adapter exists: [`github`]. It builds a repository inventory
//!   (one `Repository` node per repository) for one organization or user.
//! - It discovers no relationships (edges) yet.
//! - It has no real HTTP transport. The network is behind the
//!   [`github::Transport`] trait, and the only implementation in this crate
//!   is a test double. Nothing here has talked to the real GitHub API.
//!
//! Adapters never sleep, retry or write anywhere. They report what they
//! saw, including partial results and how long a rate limit asks the caller
//! to wait, and the caller decides what to do next.

pub mod github;
mod json;

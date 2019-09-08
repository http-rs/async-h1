//! Streaming async HTTP 1.1 parser.
//!
//! At its core HTTP is a stateful RPC protocol, where a client and server
//! communicate with one another by encoding and decoding messages between them.
//!
//! - `client` encodes HTTP requests, and decodes HTTP responses.
//! - `server` decodes HTTP requests, and encodes HTTP responses.
//!
//! ```txt
//!   encode            decode
//!        \            /
//!        -> request  ->
//! client                server
//!        <- response <-
//!        /            \
//!   decode            encode
//! ```
//!
//! See also [`async-tls`](https://docs.rs/async-tls),
//! [`async-std`](https://docs.rs/async-std).
//!
//! # Example
//!
//! ```
//! // tbi
//! ```

// ref: https://github.com/hyperium/hyper/blob/b342c38f08972fe8be4ef9844e30f1e7a121bbc4/src/proto/h1/role.rs

#![forbid(unsafe_code, future_incompatible, rust_2018_idioms)]
#![deny(missing_debug_implementations, nonstandard_style)]
#![warn(missing_docs, missing_doc_code_examples, unreachable_pub)]
#![cfg_attr(test, deny(warnings))]

/// The maximum amount of headers parsed on the server.
const MAX_HEADERS: usize = 128;

pub use body::Body;
pub use check::check;

mod body;
mod check;

pub mod server;
pub mod client;

/// A generic fallible type.
pub type Exception = Box<dyn std::error::Error + Send + Sync + 'static>;

//! Streaming async HTTP 1.1 parser.
//!
//! At its core HTTP is a stateful RPC protocol, where a client and server
//! communicate with one another by encoding and decoding messages between them.
//!
//! - `client` encodes HTTP requests, and decodes HTTP responses.
//! - `server` decodes HTTP requests, and encodes HTTP responses.
//!
//! A client always starts the HTTP connection. The lifetime of an HTTP
//! connection looks like this:
//!
//! ```txt
//! 1. encode            2. decode
//!         \            /
//!         -> request  ->
//!  client                server
//!         <- response <-
//!         /            \
//! 4. decode            3. encode
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

#![forbid(unsafe_code, future_incompatible, rust_2018_idioms)]
#![deny(missing_debug_implementations, nonstandard_style)]
// #![warn(missing_docs, missing_doc_code_examples, unreachable_pub)]
#![cfg_attr(test, deny(warnings))]

/// The maximum amount of headers parsed on the server.
const MAX_HEADERS: usize = 128;

pub use check::check;

mod check;

pub mod client;
pub mod server;

/// A generic fallible type.
pub type Exception = Box<dyn std::error::Error + Send + Sync + 'static>;

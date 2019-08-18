//! Asynchronous HTTP 1.1 parser.
//!
//! ## Example
//!
//! ```rust
//! // tbi
//! ```

// ref: https://github.com/hyperium/hyper/blob/b342c38f08972fe8be4ef9844e30f1e7a121bbc4/src/proto/h1/role.rs

#![forbid(unsafe_code, future_incompatible, rust_2018_idioms)]
#![deny(missing_debug_implementations, nonstandard_style)]
#![warn(missing_docs, missing_doc_code_examples, unreachable_pub)]
#![cfg_attr(test, deny(warnings))]

#![feature(async_await)]

use futures::prelude::*;

const MAX_HEADERS: usize = 100;

/// Body type.
#[derive(Debug)]
pub struct Body;
// impl AsyncRead for Body {}
// impl AsyncWrite for Body {}

/// Check if the protocol is HTTP/1
pub fn is_http_1(_stream: &mut impl AsyncRead) -> bool {
    unimplemented!();
}

/// Process HTTP connections on the server.
pub mod server {
    use futures::prelude::*;
    use http::{Request, Response};

    use crate::Body;

    /// Encode an HTTP request on the server.
    pub async fn encode(_res: http::Request<Body>) -> httparse::Result<Response<Body>> {
        unimplemented!();
    }

    /// Decode an HTTP request on the server.
    pub async fn decode(_stream: &mut impl AsyncRead) -> httparse::Result<Request<Body>> {
        let mut headers = [httparse::EMPTY_HEADER; crate::MAX_HEADERS];
        let mut req = httparse::Request::new(&mut headers);
        // let _res = req.parse(buf)?;
        unimplemented!();
    }
}

/// Process HTTP connections on the client.
pub mod client {
    use futures::prelude::*;

    /// Encode an HTTP request on the client.
    pub fn encode(_req: &mut impl AsyncRead, _buf: &[u8]) -> httparse::Result<Vec<u8>> {
        unimplemented!();
    }

    /// Decode an HTTP request on the client.
    pub fn decode(_res: &mut impl AsyncWrite) -> httparse::Result<Vec<u8>> {
        unimplemented!();
    }
}

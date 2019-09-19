//! Process HTTP connections on the client.

// use async_std::io::{self, BufRead, BufReader, Read};
// use async_std::task::{Context, Poll};
// use futures_io::AsyncRead;
// use http::{Request, Response};

// use std::pin::Pin;

// use crate::{Body, Exception};

/// An HTTP encoder.
#[derive(Debug)]
pub struct Encoder;

// /// Encode an HTTP request on the client.
// pub async fn encode(_res: Request<Body>) -> Result<Encoder, std::io::Error> {
//     unimplemented!();
// }

// /// Decode an HTTP request on the client.
// pub async fn decode(_reader: &mut (impl AsyncRead + Unpin)) -> Result<Response<Body>, Exception> {
//     unimplemented!();
// }

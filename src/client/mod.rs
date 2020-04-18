//! Process HTTP connections on the client.

use async_std::io::{self, Read, Write};
use http_types::{Request, Response};

use std::future::Future;
use std::pin::Pin;

mod decode;
mod encode;

pub use decode::decode;
pub use encode::Encoder;

/// HTTP/1.1 client.
#[derive(Debug)]
pub struct HttpClient;

impl HttpClient {
    /// Creates a new instance of `HttpClientBuilder`.
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::new()
    }

    // Public for testing purposes only.
    #[doc(hidden)]
    pub async fn decode<R>(reader: R) -> http_types::Result<Response>
    where
        R: Read + Unpin + Send + Sync + 'static,
    {
        decode(reader).await
    }

    /// Opens an HTTP/1.1 connection to a remote host.
    pub async fn connect<RW>(stream: RW, req: Request) -> http_types::Result<Response>
    where
        RW: Read + Write + Send + Sync + Unpin + 'static,
    {
        let builder = HttpClientBuilder::new();
        builder.connect(stream, req).await
    }
}

/// HTTP/1.1 client builder.
#[derive(Debug)]
pub struct HttpClientBuilder;

impl HttpClientBuilder {
    /// Creates a new instance of `HttpClientBuilder`.
    pub fn new() -> Self {
        Self {}
    }

    /// Opens an HTTP/1.1 connection to a remote host.
    pub async fn connect<RW>(self, mut stream: RW, req: Request) -> http_types::Result<Response>
    where
        RW: Read + Write + Send + Sync + Unpin + 'static,
    {
        let mut req = Encoder::encode(req).await?;
        log::trace!("> {:?}", &req);

        io::copy(&mut req, &mut stream).await?;

        let res = decode(stream).await?;
        log::trace!("< {:?}", &res);

        Ok(res)
    }
}

impl http_types::Client for HttpClient {
    fn send_req(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = http_service::Result<Response>> + 'static + Send>> {
        todo!();
    }
}

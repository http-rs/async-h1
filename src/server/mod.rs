//! Process HTTP connections on the server.

use std::pin::Pin;
use std::time::Duration;

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self};
use async_std::io::{Read, Write};
use http_types::{Request, Response};

mod decode;
mod encode;

use decode::decode;
use encode::Encoder;

/// HTTP/1.1 server.
#[derive(Debug)]
pub struct HttpServer<F, Fut> {
    headers_timeout: Option<Duration>,
    endpoint: F,
    __fut: std::marker::PhantomData<Fut>,
}

impl<F, Fut> HttpServer<F, Fut>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    /// Create a new `HttpServer` builder that takes various configuration.
    // pub fn builder() -> HttpServerBuilder {
    //     HttpServerBuilder::new()
    // }

    /// Create a new instance of `HttpServer`.
    pub async fn new(endpoint: F) -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(60)),
            endpoint,
            __fut: std::marker::PhantomData,
        }
    }

    /// Accept a new incoming HTTP/1.1 connection.
    ///
    /// Supports `KeepAlive` requests by default.
    pub async fn accept<RW>(self, addr: &str, io: RW) -> http_types::Result<()>
    where
        RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
        F: Fn(Request) -> Fut,
        Fut: Future<Output = http_types::Result<Response>>,
    {
        loop {
            // Decode a new request, timing out if this takes longer than the timeout duration.
            let fut = decode(self.addr, io.clone());

            let req = if let Some(timeout_duration) = self.headers_timeout {
                match timeout(timeout_duration, fut).await {
                    Ok(Ok(Some(r))) => r,
                    Ok(Ok(None)) | Err(TimeoutError { .. }) => break, /* EOF or timeout */
                    Ok(Err(e)) => return Err(e),
                }
            } else {
                match fut.await? {
                    Some(r) => r,
                    None => break, /* EOF */
                }
            };

            // Pass the request to the endpoint and encode the response.
            let res = self.endpoint(req).await?;
            let mut encoder = Encoder::encode(res);

            // Stream the response to the writer.
            io::copy(&mut encoder, &mut io).await?;
        }

        Ok(())
    }
}

impl<F, Fut> http_types::Server for HttpServer<F, Fut> {
    fn recv_req(
        &self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = http_types::Result<Response>> + 'static + Send>> {
        Box::pin(async move {
            let res = self.endpoint(req).await?;
            Ok(res)
        })
    }
}

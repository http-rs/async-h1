//! Process HTTP connections on the server.

use std::time::Duration;

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self};
use async_std::io::{Read, Write};
use http_types::{Request, Response};

mod decode;
mod encode;

use decode::decode;
use encode::Encoder;

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<RW, F, Fut>(addr: &str, mut io: RW, endpoint: F) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    // TODO: make these values configurable
    let timeout_duration = Duration::from_secs(10);
    const MAX_REQUESTS: usize = 200;
    let mut num_requests = 0;

    loop {
        // Stop parsing requests if we exceed the threshold.
        match num_requests {
            MAX_REQUESTS => return Ok(()),
            _ => num_requests += 1,
        };

        // Decode a new request, timing out if this takes longer than the
        // timeout duration.
        let req = match timeout(timeout_duration, decode(addr, io.clone())).await {
            Ok(Ok(Some(r))) => r,
            Ok(Ok(None)) | Err(TimeoutError { .. }) => break, /* EOF or timeout */
            Ok(Err(e)) => return Err(e),
        };

        // Pass the request to the endpoint and encode the response.
        let res = endpoint(req).await?;
        let mut encoder = Encoder::encode(res);

        // Stream the response to the writer.
        io::copy(&mut encoder, &mut io).await?;
    }

    Ok(())
}

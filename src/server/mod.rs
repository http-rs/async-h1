//! Process HTTP connections on the server.

use std::time::Duration;

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self, Read, Write};
use http_types::headers::{CONNECTION, UPGRADE};
use http_types::upgrade::Connection;
use http_types::{Request, Response, StatusCode};

mod decode;
mod encode;

pub use decode::decode;
pub use encode::Encoder;

/// Configure the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Timeout to handle headers. Defaults to 60s.
    headers_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(60)),
        }
    }
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<RW, F, Fut>(io: RW, endpoint: F) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    accept_with_opts(io, endpoint, Default::default()).await
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept_with_opts<RW, F, Fut>(
    mut io: RW,
    endpoint: F,
    opts: ServerOptions,
) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    // Decode a new request, timing out if this takes longer than the timeout duration.
    let fut = decode(io.clone());

    let req = if let Some(timeout_duration) = opts.headers_timeout {
        match timeout(timeout_duration, fut).await {
            Ok(Ok(Some(r))) => r,
            Ok(Ok(None)) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof").into()),
            Err(TimeoutError { .. }) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out").into())
            }
            Ok(Err(e)) => return Err(e),
        }
    } else {
        match fut.await? {
            Some(r) => r,
            None => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof").into()),
        }
    };

    let has_upgrade_header = req.header(UPGRADE).is_some();
    let connection_header = req.header(CONNECTION).map(|c| c.as_str());

    let connection_header_is_upgrade = connection_header
        .map(|connection| connection.eq_ignore_ascii_case("upgrade"))
        .unwrap_or(false);

    let upgrade_requested = has_upgrade_header && connection_header_is_upgrade;

    let method = req.method();

    // Pass the request to the endpoint and encode the response.
    let mut res = endpoint(req).await?;

    res.remove_header(CONNECTION);
    res.insert_header(CONNECTION, "close");

    let upgrade_provided = res.status() == StatusCode::SwitchingProtocols && res.has_upgrade();

    let upgrade_sender = if upgrade_requested && upgrade_provided {
        Some(res.send_upgrade())
    } else {
        None
    };

    let mut encoder = Encoder::new(res, method);

    // Stream the response to the writer.
    io::copy(&mut encoder, &mut io).await?;

    if let Some(upgrade_sender) = upgrade_sender {
        upgrade_sender.send(Connection::new(io.clone())).await;
    }
    Ok(())
}

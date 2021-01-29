//! Process HTTP connections on the server.

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self, BufRead, BufReader, Read, Write};
use http_types::headers::{CONNECTION, UPGRADE};
use http_types::upgrade::Connection;
use http_types::{Request, Response, StatusCode};
use std::{marker::PhantomData, time::Duration};

mod decode;
mod encode;

pub use decode::{decode, decode_rw};
pub use encode::Encoder;

use crate::sequenced::Sequenced;
use crate::unite::Unite;

/// Configure the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Timeout to handle headers. Defaults to 60s.
    headers_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(30)),
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
    Server::new(io, endpoint).accept().await
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept_with_opts<RW, F, Fut>(
    io: RW,
    endpoint: F,
    opts: ServerOptions,
) -> http_types::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    Server::new(io, endpoint).with_opts(opts).accept().await
}

/// struct for server
#[derive(Debug)]
pub struct Server<R, W, F, Fut> {
    reader: Sequenced<R>,
    writer: Sequenced<W>,
    endpoint: F,
    opts: ServerOptions,
    _phantom: PhantomData<Fut>,
}

/// An enum that represents whether the server should accept a subsequent request
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ConnectionStatus {
    /// The server should not accept another request
    Close,

    /// The server may accept another request
    KeepAlive,
}

impl<RW, F, Fut> Server<BufReader<RW>, RW, F, Fut>
where
    RW: Read + Write + Send + Sync + Clone + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    /// builds a new server
    pub fn new(io: RW, endpoint: F) -> Self {
        Self::new_rw(
            Sequenced::new(BufReader::new(io.clone())),
            Sequenced::new(io),
            endpoint,
        )
    }
}

impl<R, W, F, Fut> Server<R, W, F, Fut>
where
    R: BufRead + Send + Sync + Unpin + 'static,
    W: Write + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = http_types::Result<Response>>,
{
    /// builds a new server
    pub fn new_rw(reader: Sequenced<R>, writer: Sequenced<W>, endpoint: F) -> Self {
        Self {
            reader,
            writer,
            endpoint,
            opts: Default::default(),
            _phantom: PhantomData,
        }
    }

    /// with opts
    pub fn with_opts(mut self, opts: ServerOptions) -> Self {
        self.opts = opts;
        self
    }

    /// accept in a loop
    pub async fn accept(&mut self) -> http_types::Result<()> {
        while ConnectionStatus::KeepAlive == self.accept_one().await? {}
        Ok(())
    }

    /// accept one request
    pub async fn accept_one(&mut self) -> http_types::Result<ConnectionStatus> {
        // Decode a new request, timing out if this takes longer than the timeout duration.
        let fut = decode_rw(self.reader.split_seq(), self.writer.split_seq());

        let (req, notify_write) = if let Some(timeout_duration) = self.opts.headers_timeout {
            match timeout(timeout_duration, fut).await {
                Ok(Ok(Some(r))) => r,
                Ok(Ok(None)) | Err(TimeoutError { .. }) => return Ok(ConnectionStatus::Close), /* EOF or timeout */
                Ok(Err(e)) => return Err(e),
            }
        } else {
            match fut.await? {
                Some(r) => r,
                None => return Ok(ConnectionStatus::Close), /* EOF */
            }
        };

        let has_upgrade_header = req.header(UPGRADE).is_some();
        let connection_header_as_str = req
            .header(CONNECTION)
            .map(|connection| connection.as_str())
            .unwrap_or("");

        let connection_header_is_upgrade = connection_header_as_str
            .split(',')
            .any(|s| s.trim().eq_ignore_ascii_case("upgrade"));
        let mut close_connection = connection_header_as_str.eq_ignore_ascii_case("close");

        let upgrade_requested = has_upgrade_header && connection_header_is_upgrade;

        let method = req.method();

        // Pass the request to the endpoint and encode the response.
        let mut res = (self.endpoint)(req).await?;

        close_connection |= res
            .header(CONNECTION)
            .map(|c| c.as_str().eq_ignore_ascii_case("close"))
            .unwrap_or(false);

        let upgrade_provided = res.status() == StatusCode::SwitchingProtocols && res.has_upgrade();

        let upgrade_sender = if upgrade_requested && upgrade_provided {
            Some(res.send_upgrade())
        } else {
            None
        };

        let mut encoder = Encoder::new(res, method);

        // This should be dropped before we begin writing the response.
        drop(notify_write);

        let bytes_written = io::copy(&mut encoder, &mut self.writer).await?;
        log::trace!("wrote {} response bytes", bytes_written);

        async_std::task::sleep(Duration::from_millis(1)).await;

        if let Some(upgrade_sender) = upgrade_sender {
            let reader = self.reader.take_inner().await;
            let writer = self.writer.take_inner().await;
            if let (Some(reader), Some(writer)) = (reader, writer) {
                upgrade_sender
                    .send(Connection::new(Unite::new(reader, writer)))
                    .await;
            }
            return Ok(ConnectionStatus::Close);
        } else if close_connection {
            Ok(ConnectionStatus::Close)
        } else {
            Ok(ConnectionStatus::KeepAlive)
        }
    }
}

//! Process HTTP connections on the server.

use async_std::future::{timeout, Future, TimeoutError};
use async_std::io::{self, Read, Write};
use http_types::upgrade::Connection;
use http_types::{
    headers::{CONNECTION, UPGRADE},
    Version,
};
use http_types::{Request, Response, StatusCode};
use std::{marker::PhantomData, time::Duration};
mod body_reader;
mod decode;
mod encode;

pub use decode::decode;
pub use encode::Encoder;

/// Configure the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Timeout to handle headers. Defaults to 60s.
    headers_timeout: Option<Duration>,
    default_host: Option<String>,
}

impl ServerOptions {
    /// constructs a new ServerOptions with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// sets the timeout by which the headers must have been received
    pub fn with_headers_timeout(mut self, headers_timeout: Duration) -> Self {
        self.headers_timeout = Some(headers_timeout);
        self
    }

    /// Sets the default http 1.0 host for this server. If no host
    /// header is provided on an http/1.0 request, this host will be
    /// used to construct the Request Url.
    ///
    /// If this is not provided, the server will respond to all
    /// http/1.0 requests with status `505 http version not
    /// supported`, whether or not a host header is provided.
    ///
    /// The default value for this is None, and as a result async-h1
    /// is by default an http-1.1-only server.
    pub fn with_default_host(mut self, default_host: &str) -> Self {
        self.default_host = Some(default_host.into());
        self
    }
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(60)),
            default_host: None,
        }
    }
}

/// Accept a new incoming HTTP/1.1 connection.
///
/// Supports `KeepAlive` requests by default.
pub async fn accept<RW, F, Fut>(io: RW, endpoint: F) -> crate::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
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
) -> crate::Result<()>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    Server::new(io, endpoint).with_opts(opts).accept().await
}

/// struct for server
#[derive(Debug)]
pub struct Server<RW, F, Fut> {
    io: RW,
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

impl<RW, F, Fut> Server<RW, F, Fut>
where
    RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    /// builds a new server
    pub fn new(io: RW, endpoint: F) -> Self {
        Self {
            io,
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
    pub async fn accept(&mut self) -> crate::Result<()> {
        while ConnectionStatus::KeepAlive == self.accept_one().await? {}
        Ok(())
    }

    /// accept one request
    pub async fn accept_one(&mut self) -> crate::Result<ConnectionStatus>
    where
        RW: Read + Write + Clone + Send + Sync + Unpin + 'static,
        F: Fn(Request) -> Fut,
        Fut: Future<Output = Response>,
    {
        // Decode a new request, timing out if this takes longer than the timeout duration.
        let fut = decode(self.io.clone(), &self.opts);

        let (req, mut body) = if let Some(timeout_duration) = self.opts.headers_timeout {
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

        let mut close_connection = if req.version() == Some(Version::Http1_0) {
            !connection_header_as_str.eq_ignore_ascii_case("keep-alive")
        } else {
            connection_header_as_str.eq_ignore_ascii_case("close")
        };

        let upgrade_requested = has_upgrade_header && connection_header_is_upgrade;

        let method = req.method();

        // Pass the request to the endpoint and encode the response.
        let mut response = (self.endpoint)(req).await;

        close_connection |= response
            .header(CONNECTION)
            .map(|c| c.as_str().eq_ignore_ascii_case("close"))
            .unwrap_or(false);

        let upgrade_provided =
            response.status() == StatusCode::SwitchingProtocols && response.has_upgrade();

        let upgrade_sender = if upgrade_requested && upgrade_provided {
            Some(response.send_upgrade())
        } else {
            None
        };

        let mut encoder = Encoder::new(response, method);

        let bytes_written = io::copy(&mut encoder, &mut self.io).await?;
        log::trace!("wrote {} response bytes", bytes_written);

        let body_bytes_discarded = io::copy(&mut body, &mut io::sink()).await?;
        log::trace!(
            "discarded {} unread request body bytes",
            body_bytes_discarded
        );

        if let Some(upgrade_sender) = upgrade_sender {
            upgrade_sender.send(Connection::new(self.io.clone())).await;
            Ok(ConnectionStatus::Close)
        } else if close_connection {
            Ok(ConnectionStatus::Close)
        } else {
            Ok(ConnectionStatus::KeepAlive)
        }
    }
}

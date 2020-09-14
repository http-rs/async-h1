//! Process HTTP connections on the client.

use async_std::io::{self, Read, Write};
use http_types::headers::USER_AGENT;
use http_types::{Request, Response};
use lazy_static::lazy_static;

mod decode;
mod encode;

pub use decode::decode;
pub use encode::Encoder;

lazy_static! {
    static ref DEFAULT_USER_AGENT: String = format!("http-rs-h1/{}", env!("CARGO_PKG_VERSION"));
}

/// Opens an HTTP/1.1 connection to a remote host.
pub async fn connect<RW>(mut stream: RW, mut req: Request) -> http_types::Result<Response>
where
    RW: Read + Write + Send + Sync + Unpin + 'static,
{
    if let None = req.header(USER_AGENT) {
        req.insert_header(USER_AGENT, DEFAULT_USER_AGENT.as_str());
    }

    let mut req = Encoder::encode(req).await?;
    log::trace!("> {:?}", &req);

    io::copy(&mut req, &mut stream).await?;

    let res = decode(stream).await?;
    log::trace!("< {:?}", &res);

    Ok(res)
}

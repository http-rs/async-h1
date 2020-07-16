//! Process HTTP connections on the client.
use futures_io::{AsyncRead, AsyncWrite};
use futures_util::{AsyncWriteExt, AsyncReadExt};
use http_types::{Request, Response};

mod decode;
mod encode;

pub use decode::decode;
pub use encode::Encoder;
use futures_util::io::BufReader;

/// Opens an HTTP/1.1 connection to a remote host.
pub async fn connect<RW>(mut stream: RW, req: Request) -> http_types::Result<Response>
where
    RW: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    let mut req = Encoder::encode(req).await?;

    let mut req_buf = Vec::new();

    req.read_to_end(&mut req_buf).await?;
    stream.write_all(&mut req_buf).await?;
    //io::copy(&mut req, &mut stream).await?;

    let res = decode(BufReader::new(stream)).await?;

    Ok(res)
}

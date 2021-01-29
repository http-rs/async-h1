//! Process HTTP connections on the server.

use std::str::FromStr;

use async_std::io::{self, BufRead, BufReader, Read, Write};
use async_std::{prelude::*, task};
use futures_channel::oneshot;
use futures_util::{select_biased, FutureExt};
use http_types::content::ContentLength;
use http_types::headers::{EXPECT, TRANSFER_ENCODING};
use http_types::{ensure, ensure_eq, format_err};
use http_types::{Body, Method, Request, Url};

use crate::chunked::ChunkedDecoder;
use crate::read_notifier::ReadNotifier;
use crate::sequenced::Sequenced;
use crate::{MAX_HEADERS, MAX_HEAD_LENGTH};

const LF: u8 = b'\n';

/// The number returned from httparse when the request is HTTP 1.1
const HTTP_1_1_VERSION: u8 = 1;

const CONTINUE_HEADER_VALUE: &str = "100-continue";
const CONTINUE_RESPONSE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";

/// Decode an HTTP request on the server.
pub async fn decode<IO>(io: IO) -> http_types::Result<Option<(Request, impl Future)>>
where
    IO: Read + Write + Clone + Send + Sync + Unpin + 'static,
{
    let mut reader = Sequenced::new(BufReader::new(io.clone()));
    let mut writer = Sequenced::new(io);
    let res = decode_rw(reader.split_seq(), writer.split_seq()).await?;
    Ok(res.map(|(r, _)| {
        (r, async move {
            reader.take_inner().await;
            writer.take_inner().await;
        })
    }))
}

async fn discard_unread_body<R1: Read + Unpin, R2>(
    mut body_reader: Sequenced<R1>,
    mut reader: Sequenced<R2>,
) -> io::Result<()> {
    // Unpoison the body reader, as we don't require it to be in any particular state
    body_reader.cure();

    // Consume the remainder of the request body
    let body_bytes_discarded = io::copy(&mut body_reader, &mut io::sink()).await?;

    log::trace!(
        "discarded {} unread request body bytes",
        body_bytes_discarded
    );

    // Unpoison the reader, as it's easier than trying to reach into the body reader to
    // release the inner `Sequenced<T>`
    reader.cure();
    reader.release();

    Ok(())
}

#[derive(Debug)]
pub struct NotifyWrite {
    sender: Option<oneshot::Sender<()>>,
}

/// Decode an HTTP request on the server.
pub async fn decode_rw<R, W>(
    mut reader: Sequenced<R>,
    mut writer: Sequenced<W>,
) -> http_types::Result<Option<(Request, NotifyWrite)>>
where
    R: BufRead + Send + Sync + Unpin + 'static,
    W: Write + Send + Sync + Unpin + 'static,
{
    let mut buf = Vec::new();
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut httparse_req = httparse::Request::new(&mut headers);

    let mut notify_write = NotifyWrite { sender: None };

    // Keep reading bytes from the stream until we hit the end of the stream.
    loop {
        let bytes_read = reader.read_until(LF, &mut buf).await?;
        // No more bytes are yielded from the stream.
        if bytes_read == 0 {
            return Ok(None);
        }

        // Prevent CWE-400 DDOS with large HTTP Headers.
        ensure!(
            buf.len() < MAX_HEAD_LENGTH,
            "Head byte length should be less than 8kb"
        );

        // We've hit the end delimiter of the stream.
        let idx = buf.len() - 1;
        if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
            break;
        }
    }

    // Convert our header buf into an httparse instance, and validate.
    let status = httparse_req.parse(&buf)?;

    ensure!(!status.is_partial(), "Malformed HTTP head");

    // Convert httparse headers + body into a `http_types::Request` type.
    let method = httparse_req.method;
    let method = method.ok_or_else(|| format_err!("No method found"))?;

    let version = httparse_req.version;
    let version = version.ok_or_else(|| format_err!("No version found"))?;

    ensure_eq!(
        version,
        HTTP_1_1_VERSION,
        "Unsupported HTTP version 1.{}",
        version
    );

    let url = url_from_httparse_req(&httparse_req)?;

    let mut req = Request::new(Method::from_str(method)?, url);

    req.set_version(Some(http_types::Version::Http1_1));

    for header in httparse_req.headers.iter() {
        req.append_header(header.name, std::str::from_utf8(header.value)?);
    }

    let content_length = ContentLength::from_headers(&req)?;
    let transfer_encoding = req.header(TRANSFER_ENCODING);

    // Return a 400 status if both Content-Length and Transfer-Encoding headers
    // are set to prevent request smuggling attacks.
    //
    // https://tools.ietf.org/html/rfc7230#section-3.3.3
    http_types::ensure_status!(
        content_length.is_none() || transfer_encoding.is_none(),
        400,
        "Unexpected Content-Length header"
    );

    // Establish a channel to wait for the body to be read. This
    // allows us to avoid sending 100-continue in situations that
    // respond without reading the body, saving clients from uploading
    // their body.
    let (body_read_sender, body_read_receiver) = async_channel::bounded(1);

    if Some(CONTINUE_HEADER_VALUE) == req.header(EXPECT).map(|h| h.as_str()) {
        // Prevent the response being written until we've decided whether to send
        // the continue message or not.
        let mut continue_writer = writer.split_seq();

        // We can swap these later to effectively deactivate the body reader, in the event
        // that we don't ask the client to send a body.
        let mut continue_reader = reader.split_seq();
        let mut after_reader = reader.split_seq_rev();

        let (notify_tx, notify_rx) = oneshot::channel();
        notify_write.sender = Some(notify_tx);

        // If the client expects a 100-continue header, spawn a
        // task to wait for the first read attempt on the body.
        task::spawn(async move {
            // It's important that we fuse this future, or else the `select` won't
            // wake up properly if the sender is dropped.
            let mut notify_rx = notify_rx.fuse();

            let should_continue = select_biased! {
                x = body_read_receiver.recv().fuse() => x.is_ok(),
                _ = notify_rx => true,
            };

            if should_continue {
                if continue_writer.write_all(CONTINUE_RESPONSE).await.is_err() {
                    return;
                }
            } else {
                // We never asked for the body, so just allow the next
                // request to continue from our current point in the stream.
                continue_reader.swap(&mut after_reader);
            }
            // Allow the rest of the response to be written
            continue_writer.release();

            // Allow the body to be read
            continue_reader.release();

            // Allow the next request to be read (after the body, if requested, has been read)
            after_reader.release();
            // Since the sender is moved into the Body, this task will
            // finish when the client disconnects, whether or not
            // 100-continue was sent.
        });
    }

    // Check for Transfer-Encoding
    if transfer_encoding
        .map(|te| te.as_str().eq_ignore_ascii_case("chunked"))
        .unwrap_or(false)
    {
        let trailer_sender = req.send_trailers();
        let mut body_reader =
            Sequenced::new(ChunkedDecoder::new(reader.split_seq(), trailer_sender));
        req.set_body(Body::from_reader(
            ReadNotifier::new(body_reader.split_seq(), body_read_sender),
            None,
        ));
        let reader_to_cure = reader.split_seq();

        // Spawn a task to consume any part of the body which is unread
        task::spawn(async move {
            let _ = discard_unread_body(body_reader, reader_to_cure).await;
        });

        reader.release();
        writer.release();
        return Ok(Some((req, notify_write)));
    } else if let Some(len) = content_length {
        let len = len.len();
        let mut body_reader = Sequenced::new(reader.split_seq().take(len));
        req.set_body(Body::from_reader(
            ReadNotifier::new(body_reader.split_seq(), body_read_sender),
            Some(len as usize),
        ));
        let reader_to_cure = reader.split_seq();

        // Spawn a task to consume any part of the body which is unread
        task::spawn(async move {
            let _ = discard_unread_body(body_reader, reader_to_cure).await;
        });

        reader.release();
        writer.release();
        Ok(Some((req, notify_write)))
    } else {
        reader.release();
        writer.release();
        Ok(Some((req, notify_write)))
    }
}

fn url_from_httparse_req(req: &httparse::Request<'_, '_>) -> http_types::Result<Url> {
    let path = req.path.ok_or_else(|| format_err!("No uri found"))?;

    let host = req
        .headers
        .iter()
        .find(|x| x.name.eq_ignore_ascii_case("host"))
        .ok_or_else(|| format_err!("Mandatory Host header missing"))?
        .value;

    let host = std::str::from_utf8(host)?;

    if path.starts_with("http://") || path.starts_with("https://") {
        Ok(Url::parse(path)?)
    } else if path.starts_with('/') {
        Ok(Url::parse(&format!("http://{}{}", host, path))?)
    } else if req.method.unwrap().eq_ignore_ascii_case("connect") {
        Ok(Url::parse(&format!("http://{}/", path))?)
    } else {
        Err(format_err!("unexpected uri format"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn httparse_req(buf: &str, f: impl Fn(httparse::Request<'_, '_>)) {
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut res = httparse::Request::new(&mut headers[..]);
        res.parse(buf.as_bytes()).unwrap();
        f(res)
    }

    #[test]
    fn url_for_connect() {
        httparse_req(
            "CONNECT server.example.com:443 HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/");
            },
        );
    }

    #[test]
    fn url_for_host_plus_path() {
        httparse_req(
            "GET /some/resource HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/some/resource");
            },
        )
    }

    #[test]
    fn url_for_host_plus_absolute_url() {
        httparse_req(
            "GET http://domain.com/some/resource HTTP/1.1\r\nHost: server.example.com\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://domain.com/some/resource"); // host header MUST be ignored according to spec
            },
        )
    }

    #[test]
    fn url_for_conflicting_connect() {
        httparse_req(
            "CONNECT server.example.com:443 HTTP/1.1\r\nHost: conflicting.host\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/");
            },
        )
    }

    #[test]
    fn url_for_malformed_resource_path() {
        httparse_req(
            "GET not-a-url HTTP/1.1\r\nHost: server.example.com\r\n",
            |req| {
                assert!(url_from_httparse_req(&req).is_err());
            },
        )
    }

    #[test]
    fn url_for_double_slash_path() {
        httparse_req(
            "GET //double/slashes HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(
                    url.as_str(),
                    "http://server.example.com:443//double/slashes"
                );
            },
        )
    }
    #[test]
    fn url_for_triple_slash_path() {
        httparse_req(
            "GET ///triple/slashes HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(
                    url.as_str(),
                    "http://server.example.com:443///triple/slashes"
                );
            },
        )
    }

    #[test]
    fn url_for_query() {
        httparse_req(
            "GET /foo?bar=1 HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(url.as_str(), "http://server.example.com:443/foo?bar=1");
            },
        )
    }

    #[test]
    fn url_for_anchor() {
        httparse_req(
            "GET /foo?bar=1#anchor HTTP/1.1\r\nHost: server.example.com:443\r\n",
            |req| {
                let url = url_from_httparse_req(&req).unwrap();
                assert_eq!(
                    url.as_str(),
                    "http://server.example.com:443/foo?bar=1#anchor"
                );
            },
        )
    }
}

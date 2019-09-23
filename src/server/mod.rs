//! Process HTTP connections on the server.

mod encoder;

use async_std::io::{self, BufRead, BufReader};
use futures_io::AsyncRead;
use http::{Request, Response, Version};

use std::sync::{Arc, Mutex};
use std::ops::DerefMut;
use std::task::{Context, Poll};
use std::pin::Pin;

use crate::{Body, Exception, MAX_HEADERS};
use encoder::Encoder;

/// An HTTP Server
// Note: the reason why we have to write this as an Arc<Mutex> is because Rust
// doesn't have non-overlapping iterators yet. We *know* this is going to be
// yielded entirely serially, but we can't prove it to Rust. Which is a bit
// annoying for performance, but probably okay for now.
#[derive(Debug)]
pub struct Server<R: AsyncRead + Unpin> {
    reader: Arc<Mutex<BufReader<R>>>,
}

impl<R: AsyncRead + Unpin> Server<R> {
    /// Create a new instance.
    pub fn connect(reader: R) -> Self {
        Self {
            reader: Arc::new(Mutex::new(BufReader::new(reader))),
        }
    }
}

/// An HTTP Connection.
#[derive(Debug)]
pub struct Connection<R: AsyncRead + Unpin> {
    reader: Arc<Mutex<BufReader<R>>>,
}

impl<R: AsyncRead + Unpin> Iterator for Server<R> {
    type Item = Connection<R>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Connection {
            reader: self.reader.clone(),
        })
    }
}

impl<R: AsyncRead + Unpin> Connection<R> {
    /// Decode an HTTP request on the server.
    pub async fn decode(&mut self) -> Result<Option<Request<Body<BodyReader<R>>>>, Exception>
    where
        R: AsyncRead + Unpin + Send,
    {
        let mut buf = Vec::new();
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_req = httparse::Request::new(&mut headers);

        // Keep reading bytes from the stream until we hit the end of the stream.
        loop {
            let bytes_read = self.reader.lock().unwrap().read_until(b'\n', &mut buf).await?;
            // No more bytes are yielded from the stream.
            if bytes_read == 0 {
                return Ok(None);
            }

            // We've hit the end delimiter of the stream.
            let idx = buf.len() - 1;
            if idx >= 3 && &buf[idx - 3..=idx] == b"\r\n\r\n" {
                break;
            }
        }

        // Convert our header buf into an httparse instance, and validate.
        let status = httparse_req.parse(&buf)?;
        if status.is_partial() {
            dbg!(String::from_utf8(buf).unwrap());
            return Err("Malformed HTTP head".into());
        }

        // Convert httparse headers + body into a `http::Request` type.
        let mut req = Request::builder();
        for header in httparse_req.headers.iter() {
            req.header(header.name, header.value);
        }
        if let Some(method) = httparse_req.method {
            req.method(method);
        }
        if let Some(path) = httparse_req.path {
            req.uri(path);
        }
        if let Some(version) = httparse_req.version {
            req.version(match version {
                1 => Version::HTTP_11,
                _ => return Err("Unsupported HTTP version".into()),
            });
        }

        // Process the body if `Content-Length` was passed.
        let body = match httparse_req
            .headers
            .iter()
            .find(|h| h.name == "Content-Length")
        {
            Some(_header) => Body::new(BodyReader { reader: self.reader.clone() }), // TODO: use the header value
            None => Body::empty(),
        };

        // Return the request.
        Ok(Some(req.body(body)?))
    }

    /// Encode an HTTP request on the server.
    // TODO: return a reader in the response
    pub async fn encode<T>(res: Response<Body<T>>) -> io::Result<Encoder<T>>
    where
        T: AsyncRead,
    {
        use std::io::Write;
        let mut buf: Vec<u8> = vec![];

        let reason = res.status().canonical_reason().unwrap();
        let status = res.status();
        write!(&mut buf, "HTTP/1.1 {} {}\r\n", status.as_str(), reason)?;

        // If the body isn't streaming, we can set the content-length ahead of time. Else we need to
        // send all items in chunks.
        if let Some(len) = res.body().len() {
            write!(&mut buf, "Content-Length: {}\r\n", len)?;
        } else {
            write!(&mut buf, "Transfer-Encoding: chunked\r\n")?;
            panic!("chunked encoding is not implemented yet");
            // See: https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Transfer-Encoding
            //      https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Trailer
        }

        for (header, value) in res.headers() {
            write!(
                &mut buf,
                "{}: {}\r\n",
                header.as_str(),
                value.to_str().unwrap()
            )?;
        }

        write!(&mut buf, "\r\n")?;
        Ok(Encoder::new(buf, res.into_body()))
    }
}

/// The reader that's passed to the body.
#[derive(Debug)]
pub struct BodyReader<R: AsyncRead + Unpin> {
    reader: Arc<Mutex<BufReader<R>>>,
}

impl<R: AsyncRead + Unpin> AsyncRead for BodyReader<R> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        panic!();
    }
}
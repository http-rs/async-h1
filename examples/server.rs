use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use async_h1::server;
use async_std::io::{self, Read, Write};
use async_std::net::{self, TcpStream};
use async_std::prelude::*;
use async_std::task::{self, Context, Poll};
use http_types::headers::{HeaderName, HeaderValue};
use http_types::{Response, StatusCode};

async fn accept(addr: String, stream: TcpStream) -> Result<(), async_h1::Exception> {
    // println!("starting new connection from {}", stream.peer_addr()?);

    // TODO: Delete this line when we implement `Clone` for `TcpStream`.
    let stream = Stream(Arc::new(stream));

    server::accept(&addr, stream.clone(), stream, |_| {
        async {
            let mut resp = Response::new(StatusCode::Ok);
            resp.insert_header(
                HeaderName::from_str("Content-Type")?,
                HeaderValue::from_str("text/plain")?,
            )?;
            resp.set_body("Hello");
            // To try chunked encoding, replace `set_body_string` with the following method call
            // .set_body(io::Cursor::new(vec![
            //     0x48u8, 0x65, 0x6C, 0x6C, 0x6F, 0x20, 0x77, 0x6F, 0x72, 0x6C, 0x64, 0x21,
            // ]));
            Ok(resp)
        }
    })
    .await
}

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        let addr = format!("http://{}", listener.local_addr()?);
        println!("listening on {}", addr);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            let stream = stream?;
            let addr = addr.clone();
            task::spawn(async {
                if let Err(err) = accept(addr, stream).await {
                    eprintln!("{}", err);
                }
            });
        }
        Ok(())
    })
}

#[derive(Clone)]
struct Stream(Arc<TcpStream>);

impl Read for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.0).poll_read(cx, buf)
    }
}

impl Write for Stream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_close(cx)
    }
}

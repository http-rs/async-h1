use std::pin::Pin;
use std::sync::Arc;

use async_h1::server;
use async_std::io::{self, Read, Write};
use async_std::net::{self, TcpStream};
use async_std::prelude::*;
use async_std::task::{self, Context, Poll};
use http_types::{Response, StatusCode};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            task::spawn(async {
                let stream = stream?;
                println!("starting new connection from {}", stream.peer_addr()?);

                // TODO: Delete this line when we implement `Clone` for `TcpStream`.
                let stream = Stream(Arc::new(stream));

                server::connect(stream.clone(), stream, |_| {
                    async { Ok(Response::new(StatusCode::Ok)) }
                })
                .await
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

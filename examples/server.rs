use async_h1::server;
use async_std::net;
use async_std::prelude::*;
use async_std::task;
use http_types::{HttpVersion, Response, StatusCode};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let listener = net::TcpListener::bind(("127.0.0.1", 8080)).await?;
        println!("listening on {}", listener.local_addr()?);
        let mut incoming = listener.incoming();

        while let Some(stream) = incoming.next().await {
            task::spawn(async {
                let stream = stream?;
                println!("starting new connection from {}", stream.peer_addr()?);

                let stream = Stream::new(stream);
                server::connect(stream.clone(), stream, |_| {
                    async { Ok(Response::new(HttpVersion::HTTP1_1, StatusCode::Ok)) }
                })
                .await
            });
        }
        Ok(())
    })
}

use async_std::{
    io::{self, Read, Write},
    net::TcpStream,
    task::{Context, Poll},
};
use std::{
    pin::Pin,
    sync::{Arc, Mutex},
};

struct Stream {
    internal: Arc<Mutex<TcpStream>>,
}

impl Stream {
    fn new(internal: TcpStream) -> Self {
        Stream {
            internal: Arc::new(Mutex::new(internal)),
        }
    }
}

impl Clone for Stream {
    fn clone(&self) -> Self {
        Stream {
            internal: self.internal.clone(),
        }
    }
}

impl Read for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        <TcpStream as Read>::poll_read(Pin::new(&mut self.internal.lock().unwrap()), cx, buf)
    }
}
impl Write for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        <TcpStream as Write>::poll_write(Pin::new(&mut self.internal.lock().unwrap()), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        <TcpStream as Write>::poll_flush(Pin::new(&mut self.internal.lock().unwrap()), cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        <TcpStream as Write>::poll_close(Pin::new(&mut self.internal.lock().unwrap()), cx)
    }
}

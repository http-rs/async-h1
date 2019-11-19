use async_h1::client;
use async_std::{io, net, task};
use http_types::{Method, Request, Url};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let tcp_stream = net::TcpStream::connect("127.0.0.1:8080").await?;
        println!("connecting to {}", tcp_stream.peer_addr()?);

        let stream = Stream::new(tcp_stream);
        for i in 0usize..2 {
            println!("making request {}/2", i + 1);

            let mut req =
                client::encode(Request::new(Method::Get, Url::parse("/foo").unwrap())).await?;
            io::copy(&mut req, &mut stream.clone()).await?;

            // read the response
            let res = client::decode(stream.clone()).await?;
            println!("{:?}", res);
        }
        Ok(())
    })
}

use async_std::{
    io::{Read, Write},
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

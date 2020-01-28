use std::pin::Pin;
use std::sync::Arc;

use async_h1::client;
use async_std::io::{self, Read, Write};
use async_std::net::{self, TcpStream};
use async_std::task::{self, Context, Poll};
use http_types::{Method, Request, Url};

fn main() -> Result<(), async_h1::Exception> {
    task::block_on(async {
        let stream = net::TcpStream::connect("127.0.0.1:8080").await?;
        let peer_addr = stream.peer_addr()?;
        println!("connecting to {}", peer_addr);

        // TODO: Delete this line when we implement `Clone` for `TcpStream`.
        let stream = Stream(Arc::new(stream));

        for i in 0usize..2 {
            println!("making request {}/2", i + 1);
            let url = Url::parse(&format!("http://{}/foo", peer_addr)).unwrap();
            let req = Request::new(Method::Get, dbg!(url));
            let res = client::connect(stream.clone(), req).await?;
            println!("{:?}", res);
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

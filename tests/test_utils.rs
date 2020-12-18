use async_h1::{
    client::Encoder,
    server::{ConnectionStatus, Server},
};
use async_std::io::{Read, Write};
use http_types::{Request, Response};
use std::{
    fmt::{Debug, Display},
    future::Future,
    io,
    pin::Pin,
    sync::RwLock,
    task::{Context, Poll, Waker},
};

use async_dup::Arc;

#[pin_project::pin_project]
pub struct TestServer<F, Fut> {
    server: Server<TestIO, F, Fut>,
    #[pin]
    client: TestIO,
}

impl<F, Fut> TestServer<F, Fut>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    #[allow(dead_code)]
    pub fn new(f: F) -> Self {
        let (client, server) = TestIO::new();
        Self {
            server: Server::new(server, f),
            client,
        }
    }

    #[allow(dead_code)]
    pub async fn accept_one(&mut self) -> async_h1::Result<ConnectionStatus> {
        self.server.accept_one().await
    }

    #[allow(dead_code)]
    pub fn close(&mut self) {
        self.client.close();
    }

    #[allow(dead_code)]
    pub fn all_read(&self) -> bool {
        self.client.all_read()
    }

    #[allow(dead_code)]
    pub async fn write_request(&mut self, request: Request) -> io::Result<()> {
        async_std::io::copy(&mut Encoder::new(request), self).await?;
        Ok(())
    }
}

impl<F, Fut> Read for TestServer<F, Fut>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.project().client.poll_read(cx, buf)
    }
}

impl<F, Fut> Write for TestServer<F, Fut>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.project().client.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().client.poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().client.poll_close(cx)
    }
}

/// a Test IO
#[derive(Default, Clone, Debug)]
pub struct TestIO {
    pub read: Arc<CloseableCursor>,
    pub write: Arc<CloseableCursor>,
}

#[derive(Default)]
pub struct CloseableCursor {
    data: RwLock<Vec<u8>>,
    cursor: RwLock<usize>,
    waker: RwLock<Option<Waker>>,
    closed: RwLock<bool>,
}

impl CloseableCursor {
    fn len(&self) -> usize {
        self.data.read().unwrap().len()
    }

    fn cursor(&self) -> usize {
        *self.cursor.read().unwrap()
    }

    fn current(&self) -> bool {
        self.len() == self.cursor()
    }

    fn close(&self) {
        *self.closed.write().unwrap() = true;
    }
}

impl Display for CloseableCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data = &*self.data.read().unwrap();
        let s = std::str::from_utf8(data).unwrap_or("not utf8");
        write!(f, "{}", s)
    }
}

impl TestIO {
    pub fn new() -> (TestIO, TestIO) {
        let client = Arc::new(CloseableCursor::default());
        let server = Arc::new(CloseableCursor::default());

        (
            TestIO {
                read: client.clone(),
                write: server.clone(),
            },
            TestIO {
                read: server,
                write: client,
            },
        )
    }

    pub fn all_read(&self) -> bool {
        self.write.current()
    }

    pub fn close(&mut self) {
        self.write.close();
    }
}

impl Debug for CloseableCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloseableCursor")
            .field(
                "data",
                &std::str::from_utf8(&self.data.read().unwrap()).unwrap_or("not utf8"),
            )
            .field("closed", &*self.closed.read().unwrap())
            .field("cursor", &*self.cursor.read().unwrap())
            .finish()
    }
}

impl Read for &CloseableCursor {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let len = self.len();
        let cursor = self.cursor();
        if cursor < len {
            let data = &*self.data.read().unwrap();
            let bytes_to_copy = buf.len().min(len - cursor);
            buf[..bytes_to_copy].copy_from_slice(&data[cursor..cursor + bytes_to_copy]);
            *self.cursor.write().unwrap() += bytes_to_copy;
            Poll::Ready(Ok(bytes_to_copy))
        } else if *self.closed.read().unwrap() {
            Poll::Ready(Ok(0))
        } else {
            *self.waker.write().unwrap() = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl Write for &CloseableCursor {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if *self.closed.read().unwrap() {
            Poll::Ready(Ok(0))
        } else {
            self.data.write().unwrap().extend_from_slice(buf);
            if let Some(waker) = self.waker.write().unwrap().take() {
                waker.wake();
            }
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(waker) = self.waker.write().unwrap().take() {
            waker.wake();
        }
        *self.closed.write().unwrap() = true;
        Poll::Ready(Ok(()))
    }
}

impl Read for TestIO {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.read).poll_read(cx, buf)
    }
}

impl Write for TestIO {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.write).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.write).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.write).poll_close(cx)
    }
}

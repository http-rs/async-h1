use std::{
    fmt, io,
    pin::Pin,
    task::{Context, Poll},
};

use async_std::{
    io::{BufRead, Read},
    sync::Sender,
};

pin_project_lite::pin_project! {
    pub(crate) struct ReadNotifier<B>{
        #[pin]
        reader: B,
        sender: Sender<()>,
        read: bool
    }
}

impl<B> fmt::Debug for ReadNotifier<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadNotifier")
            .field("read", &self.read)
            .finish()
    }
}

impl<B: BufRead> ReadNotifier<B> {
    pub(crate) fn new(reader: B, sender: Sender<()>) -> Self {
        Self {
            reader,
            sender,
            read: false,
        }
    }
}

impl<B: BufRead> BufRead for ReadNotifier<B> {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<&[u8]>> {
        self.project().reader.poll_fill_buf(cx)
    }

    fn consume(self: Pin<&mut Self>, amt: usize) {
        self.project().reader.consume(amt)
    }
}

impl<B: Read> Read for ReadNotifier<B> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();

        if !*this.read {
            if let Ok(()) = this.sender.try_send(()) {
                *this.read = true;
            };
        }

        this.reader.poll_read(cx, buf)
    }
}

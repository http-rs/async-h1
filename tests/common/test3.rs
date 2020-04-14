use async_std::fs::File;
use async_std::io::prelude::*;
use async_std::io::{self, SeekFrom};
use async_std::sync::Arc;
use async_std::task::{Context, Poll};
use std::pin::Pin;
use std::sync::Mutex;

#[derive(Clone)]
pub struct TestCase {
    reader: Arc<Mutex<File>>,
    writer: Arc<Mutex<File>>,
}

impl TestCase {
    /// Create a new instance.
    pub async fn new(reader: &str, writer: &str) -> TestCase {
        use std::io::Write;

        let mut temp = tempfile::tempfile().expect("Failed writer create tempfile");
        temp.write(reader.as_bytes())
            .expect("Could not write writer dest file");
        let mut file: File = temp.into();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        let reader = Arc::new(Mutex::new(file.into()));

        let mut temp = tempfile::tempfile().expect("Failed writer create tempfile");
        temp.write(writer.as_bytes())
            .expect("Could not write writer dest file");
        let mut file: File = temp.into();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        let writer = Arc::new(Mutex::new(file.into()));

        TestCase { reader, writer }
    }

    /// Get the value of the "writer" string.
    pub async fn writer(&self) -> String {
        let mut writer = String::new();
        let mut file = self.writer.lock().unwrap();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        file.read_to_string(&mut writer).await.unwrap();
        writer
    }

    /// Assert the reader.
    pub async fn assert_reader(&self, rhs: &str) {
        let mut lhs = self.reader().await;
        pretty_assertions::assert_eq!(lhs, rhs);
    }

    /// Assert the reader but pass it through a closure first..
    pub async fn assert_reader_with(&self, mut rhs: &str, f: impl Fn(&mut String, &mut String)) {
        let mut lhs = self.reader().await;
        let mut rhs = String::from(rhs);
        f(&mut lhs, &mut rhs);
        pretty_assertions::assert_eq!(lhs, rhs);
    }

    /// Assert the writer.
    pub async fn assert_writer(&self, rhs: &str) {
        let mut lhs = self.writer().await;
        pretty_assertions::assert_eq!(lhs, rhs);
    }

    /// Assert the reader but pass it through a closure first..
    pub async fn assert_writer_with(&self, mut rhs: &str, f: impl Fn(&mut String, &mut String)) {
        let mut lhs = self.writer().await;
        let mut rhs = String::from(rhs);
        f(&mut lhs, &mut rhs);
        pretty_assertions::assert_eq!(lhs, rhs);
    }

    /// Get the value of the "reader" string.
    pub async fn reader(&self) -> String {
        let mut reader = String::new();
        let mut file = self.reader.lock().unwrap();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        file.read_to_string(&mut reader).await.unwrap();
        reader
    }
}

impl Read for TestCase {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.reader.lock().unwrap()).poll_read(cx, buf)
    }
}

impl Write for TestCase {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.writer.lock().unwrap()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.writer.lock().unwrap()).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.writer.lock().unwrap()).poll_close(cx)
    }
}

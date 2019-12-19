use async_std::fs::File;
use async_std::fs::OpenOptions;
use async_std::io::{self, Read, SeekFrom, Write};
use async_std::path::PathBuf;
use async_std::sync::Arc;
use async_std::task::{Context, Poll};
use std::pin::Pin;
use std::sync::Mutex;

#[macro_export]
macro_rules! assert {
    ($actual:expr, $expected_file:expr, $block:expr) => {
        task::block_on(async {
            use async_std::io::prelude::*;
            $block.await.unwrap();
            let mut actual = $actual.to_string().await;
            let mut expected = std::string::String::new();
            $expected_file.read_to_string(&mut expected).await.unwrap();
            match expected.find("{DATE}") {
                Some(i) => {
                    expected.replace_range(i..i + 6, "");
                    match expected.get(i..i + 1) {
                        Some(byte) => {
                            let j = actual[i..].find(byte).expect("Byte not found");
                            actual.replace_range(i..i + j, "");
                        }
                        None => expected.replace_range(i.., ""),
                    }
                }
                None => {}
            }
            pretty_assertions::assert_eq!(actual, expected);
        })
    };
}

pub async fn read_fixture(name: &str) -> TestFile {
    let directory: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let path: PathBuf = format!("tests/fixtures/{}.txt", name).into();
    let file = File::open(directory.join(path))
        .await
        .expect("Reading fixture file didn't work");
    let temp = std::env::temp_dir().join("foo.txt");
    let temp = OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp)
        .await
        .unwrap();
    TestFile(Arc::new(file), Arc::new(Mutex::new(temp)))
}

#[derive(Clone)]
pub struct TestFile(Arc<File>, Arc<Mutex<File>>);

impl TestFile {
    pub async fn to_string(self) -> String {
        use async_std::prelude::*;
        let mut buf = String::new();
        let mut file = self.1.lock().unwrap();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        dbg!(file.read_to_string(&mut buf).await.unwrap());
        buf
    }
}

impl Read for TestFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.0).poll_read(cx, buf)
    }
}

impl Write for TestFile {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.1.lock().unwrap()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.1.lock().unwrap()).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.1.lock().unwrap()).poll_close(cx)
    }
}

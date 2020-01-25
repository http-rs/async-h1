use async_std::fs::File;
use async_std::io::{self, Read, SeekFrom, Write};
use async_std::path::PathBuf;
use async_std::sync::Arc;
use async_std::task::{Context, Poll};
use std::pin::Pin;
use std::sync::Mutex;

#[derive(Clone)]
pub struct TestCase {
    request_fixture: Arc<File>,
    response_fixture: Arc<Mutex<File>>,
    result: Arc<Mutex<File>>,
}

impl TestCase {
    pub async fn new(request_file_path: &str, response_file_path: &str) -> TestCase {
        let request_fixture = File::open(fixture_path(&request_file_path))
            .await
            .expect(&format!(
                "Could not open request fixture file: {:?}",
                &fixture_path(request_file_path)
            ));
        let request_fixture = Arc::new(request_fixture);

        let response_fixture =
            File::open(fixture_path(&response_file_path))
                .await
                .expect(&format!(
                    "Could not open response fixture file: {:?}",
                    &fixture_path(response_file_path)
                ));
        let response_fixture = Arc::new(Mutex::new(response_fixture));

        let temp = tempfile::tempfile().expect("Failed to create tempfile");
        let result = Arc::new(Mutex::new(temp.into()));

        TestCase {
            request_fixture,
            response_fixture,
            result,
        }
    }

    pub async fn read_result(&self) -> String {
        use async_std::prelude::*;
        let mut result = String::new();
        let mut file = self.result.lock().unwrap();
        file.seek(SeekFrom::Start(0)).await.unwrap();
        file.read_to_string(&mut result).await.unwrap();
        result
    }

    pub async fn read_expected(&self) -> String {
        use async_std::prelude::*;
        let mut expected = std::string::String::new();
        self.response_fixture
            .lock()
            .unwrap()
            .read_to_string(&mut expected)
            .await
            .unwrap();
        expected
    }

    pub async fn assert(self) {
        let mut actual = self.read_result().await;
        let mut expected = self.read_expected().await;
        assert!(!actual.is_empty(), "Received empty reply");
        assert!(!expected.is_empty(), "Missing expected fixture");

        // munge actual and expected so that we don't rely on dates matching exactly
        munge_date(&mut expected, &mut actual);
        pretty_assertions::assert_eq!(actual, expected);
    }
}

pub(crate) fn fixture_path(relative_path: &str) -> PathBuf {
    let directory: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    directory.join("tests").join(relative_path)
}

pub(crate) fn munge_date(expected: &mut String, actual: &mut String) {
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
}

impl Read for TestCase {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.request_fixture).poll_read(cx, buf)
    }
}

impl Write for TestCase {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.result.lock().unwrap()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.result.lock().unwrap()).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.result.lock().unwrap()).poll_close(cx)
    }
}

use async_dup::{Arc, Mutex};
use async_std::io::{Cursor, SeekFrom};
use async_std::{prelude::*, task};
use duplexify::Duplex;
use http_types::Result;
use std::time::Duration;

const REQUEST_WITH_EXPECT: &[u8] = b"POST / HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Length: 10\r\n\
Expect: 100-continue\r\n\r\n";

const SLEEP_DURATION: Duration = std::time::Duration::from_millis(100);
#[async_std::test]
async fn test_with_expect_when_reading_body() -> Result<()> {
    let client_str: Vec<u8> = REQUEST_WITH_EXPECT.to_vec();
    let server_str: Vec<u8> = vec![];

    let mut client = Arc::new(Mutex::new(Cursor::new(client_str)));
    let server = Arc::new(Mutex::new(Cursor::new(server_str)));

    let mut request = async_h1::server::decode(Duplex::new(client.clone(), server.clone()))
        .await?
        .unwrap();

    task::sleep(SLEEP_DURATION).await; //prove we're not just testing before we've written

    {
        let lock = server.lock();
        assert_eq!("", std::str::from_utf8(lock.get_ref())?); //we haven't written yet
    };

    let mut buf = vec![0u8; 1];
    let bytes = request.read(&mut buf).await?; //this triggers the 100-continue even though there's nothing to read yet
    assert_eq!(bytes, 0); // normally we'd actually be waiting for the end of the buffer, but this lets us test this sequentially

    task::sleep(SLEEP_DURATION).await; // just long enough to wait for the channel and io

    {
        let lock = server.lock();
        assert_eq!(
            "HTTP/1.1 100 Continue\r\n\r\n",
            std::str::from_utf8(lock.get_ref())?
        );
    };

    client.write_all(b"0123456789").await?;
    client
        .seek(SeekFrom::Start(REQUEST_WITH_EXPECT.len() as u64))
        .await?;

    assert_eq!("0123456789", request.body_string().await?);

    Ok(())
}

#[async_std::test]
async fn test_without_expect_when_not_reading_body() -> Result<()> {
    let client_str: Vec<u8> = REQUEST_WITH_EXPECT.to_vec();
    let server_str: Vec<u8> = vec![];

    let client = Arc::new(Mutex::new(Cursor::new(client_str)));
    let server = Arc::new(Mutex::new(Cursor::new(server_str)));

    async_h1::server::decode(Duplex::new(client.clone(), server.clone()))
        .await?
        .unwrap();

    task::sleep(SLEEP_DURATION).await; // just long enough to wait for the channel

    let server_lock = server.lock();
    assert_eq!("", std::str::from_utf8(server_lock.get_ref())?); // we haven't written 100-continue

    Ok(())
}

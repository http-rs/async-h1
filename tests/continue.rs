mod test_utils;

use async_h1::server::ConnectionStatus;
use async_std::future::timeout;
use async_std::io::BufReader;
use async_std::{io, prelude::*, task};
use http_types::{Response, Result};
use std::time::Duration;
use test_utils::{TestIO, TestServer};

const REQUEST_WITH_EXPECT: &[u8] = b"POST / HTTP/1.1\r\n\
Host: example.com\r\n\
Content-Length: 10\r\n\
Expect: 100-continue\r\n\r\n";

const SLEEP_DURATION: Duration = std::time::Duration::from_millis(100);
#[async_std::test]
async fn test_with_expect_when_reading_body() -> Result<()> {
    let (mut client, server) = TestIO::new();
    client.write_all(REQUEST_WITH_EXPECT).await?;

    let (mut request, _) = async_h1::server::decode(server).await?.unwrap();

    task::sleep(SLEEP_DURATION).await; //prove we're not just testing before we've written

    assert_eq!("", &client.read.to_string()); // we haven't written yet

    let join_handle = task::spawn(async move {
        let mut string = String::new();
        request.read_to_string(&mut string).await?; //this triggers the 100-continue even though there's nothing to read yet
        io::Result::Ok(string)
    });

    task::sleep(SLEEP_DURATION).await; // just long enough to wait for the channel and io

    assert_eq!("HTTP/1.1 100 Continue\r\n\r\n", &client.read.to_string());

    client.write_all(b"0123456789").await?;

    assert_eq!("0123456789", &join_handle.await?);

    Ok(())
}

#[async_std::test]
async fn test_without_expect_when_not_reading_body() -> Result<()> {
    let (mut client, server) = TestIO::new();
    client.write_all(REQUEST_WITH_EXPECT).await?;

    let (_, _) = async_h1::server::decode(server).await?.unwrap();

    task::sleep(SLEEP_DURATION).await; // just long enough to wait for the channel

    assert_eq!("", &client.read.to_string()); // we haven't written 100-continue

    Ok(())
}

#[async_std::test]
async fn test_accept_unread_body() -> Result<()> {
    let mut server = TestServer::new(|_| async { Ok(Response::new(200)) });

    server.write_all(REQUEST_WITH_EXPECT).await?;
    assert_eq!(
        timeout(Duration::from_secs(1), server.accept_one()).await??,
        ConnectionStatus::KeepAlive
    );

    server.write_all(REQUEST_WITH_EXPECT).await?;
    assert_eq!(
        timeout(Duration::from_secs(1), server.accept_one()).await??,
        ConnectionStatus::KeepAlive
    );

    server.close();
    assert_eq!(server.accept_one().await?, ConnectionStatus::Close);

    assert!(server.all_read());

    Ok(())
}

#[async_std::test]
async fn test_echo_server() -> Result<()> {
    let mut server = TestServer::new(|mut req| async move {
        let mut resp = Response::new(200);
        resp.set_body(req.take_body());
        Ok(resp)
    });

    server.write_all(REQUEST_WITH_EXPECT).await?;
    server.write_all(b"0123456789").await?;
    assert_eq!(server.accept_one().await?, ConnectionStatus::KeepAlive);

    task::sleep(SLEEP_DURATION).await; // wait for "continue" to be sent

    server.close();

    assert!(server
        .client
        .read
        .to_string()
        .starts_with("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n"));

    assert_eq!(server.accept_one().await?, ConnectionStatus::Close);

    assert!(server.all_read());

    Ok(())
}

#[async_std::test]
async fn test_delayed_read() -> Result<()> {
    let mut server = TestServer::new(|mut req| async move {
        let mut body = req.take_body();
        task::spawn(async move {
            let mut buf = Vec::new();
            body.read_to_end(&mut buf).await.unwrap();
        });
        Ok(Response::new(200))
    });

    server.write_all(REQUEST_WITH_EXPECT).await?;
    assert_eq!(
        timeout(Duration::from_secs(1), server.accept_one()).await??,
        ConnectionStatus::KeepAlive
    );
    server.write_all(b"0123456789").await?;

    server.write_all(REQUEST_WITH_EXPECT).await?;
    assert_eq!(
        timeout(Duration::from_secs(1), server.accept_one()).await??,
        ConnectionStatus::KeepAlive
    );
    server.write_all(b"0123456789").await?;

    server.close();
    assert_eq!(server.accept_one().await?, ConnectionStatus::Close);

    assert!(server.all_read());

    Ok(())
}

#[async_std::test]
async fn test_accept_fast_unread_sequential_requests() -> Result<()> {
    let mut server = TestServer::new(|_| async move { Ok(Response::new(200)) });
    let mut client = server.client.clone();

    task::spawn(async move {
        let mut reader = BufReader::new(client.clone());
        for _ in 0..10 {
            let mut buf = String::new();
            client.write_all(REQUEST_WITH_EXPECT).await.unwrap();

            while !buf.ends_with("\r\n\r\n") {
                reader.read_line(&mut buf).await.unwrap();
            }

            assert!(buf.starts_with("HTTP/1.1 200 OK\r\n"));
        }
        client.close();
    });

    for _ in 0..10 {
        assert_eq!(
            timeout(Duration::from_secs(1), server.accept_one()).await??,
            ConnectionStatus::KeepAlive
        );
    }

    assert_eq!(server.accept_one().await?, ConnectionStatus::Close);

    assert!(server.all_read());

    Ok(())
}

#[async_std::test]
async fn test_accept_partial_read_sequential_requests() -> Result<()> {
    const LARGE_REQUEST_WITH_EXPECT: &[u8] = b"POST / HTTP/1.1\r\n\
        Host: example.com\r\n\
        Content-Length: 1000\r\n\
        Expect: 100-continue\r\n\r\n";

    let mut server = TestServer::new(|mut req| async move {
        let mut body = req.take_body();
        let mut buf = [0];
        body.read(&mut buf).await.unwrap();
        Ok(Response::new(200))
    });
    let mut client = server.client.clone();

    task::spawn(async move {
        let mut reader = BufReader::new(client.clone());
        for _ in 0..10 {
            let mut buf = String::new();
            client.write_all(LARGE_REQUEST_WITH_EXPECT).await.unwrap();

            // Wait for body to be requested
            while !buf.ends_with("\r\n\r\n") {
                reader.read_line(&mut buf).await.unwrap();
            }
            assert!(buf.starts_with("HTTP/1.1 100 Continue\r\n"));

            // Write body
            for _ in 0..100 {
                client.write_all(b"0123456789").await.unwrap();
            }

            // Wait for response
            buf.clear();
            while !buf.ends_with("\r\n\r\n") {
                reader.read_line(&mut buf).await.unwrap();
            }

            assert!(buf.starts_with("HTTP/1.1 200 OK\r\n"));
        }
        client.close();
    });

    for _ in 0..10 {
        assert_eq!(
            timeout(Duration::from_secs(1), server.accept_one()).await??,
            ConnectionStatus::KeepAlive
        );
    }

    assert_eq!(
        timeout(Duration::from_secs(1), server.accept_one()).await??,
        ConnectionStatus::Close
    );

    assert!(server.all_read());

    Ok(())
}

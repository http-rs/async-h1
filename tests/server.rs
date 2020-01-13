use async_h1::server;
use async_std::io::Cursor;
use async_std::prelude::*;
use common::TestCase;
use http_types::{mime, Body, Response, StatusCode};

mod common;

#[async_std::test]
async fn test_basic_request() {
    let case = TestCase::new("fixtures/request1.txt", "fixtures/response1.txt").await;
    let addr = "http://example.com";

    server::accept(addr, case.clone(), |_req| async {
        let mut resp = Response::new(StatusCode::Ok);
        resp.set_body("");
        Ok(resp)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_chunked_basic() {
    let case = TestCase::new(
        "fixtures/request-chunked-basic.txt",
        "fixtures/response-chunked-basic.txt",
    )
    .await;
    let addr = "http://example.com";

    server::accept(addr, case.clone(), |_req| async {
        let mut resp = Response::new(StatusCode::Ok);
        resp.set_body(Body::from_reader(
            Cursor::new(b"Mozilla")
                .chain(Cursor::new(b"Developer"))
                .chain(Cursor::new(b"Network")),
            None,
        ));
        resp.set_content_type(mime::PLAIN);
        Ok(resp)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_chunked_echo() {
    let case = TestCase::new(
        "fixtures/request-chunked-echo.txt",
        "fixtures/response-chunked-echo.txt",
    )
    .await;
    let addr = "http://example.com";

    server::accept(addr, case.clone(), |req| async {
        let mut resp = Response::new(StatusCode::Ok);
        let ct = req.content_type();
        let body: Body = req.into();
        resp.set_body(body);
        if let Some(ct) = ct {
            resp.set_content_type(ct);
        }

        Ok(resp)
    })
    .await
    .unwrap();

    case.assert().await;
}

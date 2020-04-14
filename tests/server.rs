use async_std::io::Cursor;
use async_std::prelude::*;
use common::TestCase;
use http_types::{mime, Body, Response, StatusCode};

mod common;

#[async_std::test]
async fn test_basic_request() {
    let case =
        TestCase::new_server("fixtures/add-date-req.http", "fixtures/add-date-res.http").await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |_req| async {
        let mut res = Response::new(StatusCode::Ok);
        res.set_body("");
        Ok(res)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_chunked_basic() {
    let case = TestCase::new_server(
        "fixtures/chunked-basic-req.http",
        "fixtures/chunked-basic-res.http",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |_req| async {
        let mut res = Response::new(StatusCode::Ok);
        res.set_body(Body::from_reader(
            Cursor::new(b"Mozilla")
                .chain(Cursor::new(b"Developer"))
                .chain(Cursor::new(b"Network")),
            None,
        ));
        res.set_content_type(mime::PLAIN);
        Ok(res)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_chunked_echo() {
    let case = TestCase::new_server(
        "fixtures/chunked-echo-req.http",
        "fixtures/chunked-echo-res.http",
    )
    .await;

    let addr = "http://example.com";
    async_h1::accept(addr, case.clone(), |req| async {
        let ct = req.content_type();
        let body: Body = req.into();

        let mut res = Response::new(StatusCode::Ok);
        res.set_body(body);
        if let Some(ct) = ct {
            res.set_content_type(ct);
        }

        Ok(res)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_unexpected_eof() {
    // We can't predict unexpected EOF, so the response content-length is still 11
    let case = TestCase::new_server(
        "fixtures/unexpected-eof-req.http",
        "fixtures/unexpected-eof-res.http",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
        let mut res = Response::new(StatusCode::Ok);
        let ct = req.content_type();
        let body: Body = req.into();
        res.set_body(body);
        if let Some(ct) = ct {
            res.set_content_type(ct);
        }

        Ok(res)
    })
    .await
    .unwrap();

    case.assert().await;
}

#[async_std::test]
async fn test_invalid_trailer() {
    let case = TestCase::new_server(
        "fixtures/invalid-trailer-req.http",
        "fixtures/invalid-trailer-res.http",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
        let mut res = Response::new(StatusCode::Ok);
        let ct = req.content_type();
        let body: Body = req.into();
        res.set_body(body);
        if let Some(ct) = ct {
            res.set_content_type(ct);
        }

        Ok(res)
    })
    .await
    .unwrap_err();

    assert!(case.read_result().await.is_empty());
}

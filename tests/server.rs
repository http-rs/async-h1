use async_std::io::Cursor;
use async_std::prelude::*;
use common::TestCase;
use http_types::{mime, Body, Response, StatusCode};

mod common;

#[async_std::test]
async fn test_basic_request() {
    let case = TestCase::new_server(
        "fixtures/request-add-date.txt",
        "fixtures/response-add-date.txt",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |_req| async {
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
    let case = TestCase::new_server(
        "fixtures/request-chunked-basic.txt",
        "fixtures/response-chunked-basic.txt",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |_req| async {
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
    let case = TestCase::new_server(
        "fixtures/request-chunked-echo.txt",
        "fixtures/response-chunked-echo.txt",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
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

#[async_std::test]
async fn test_chunked_echo_throttled() {
    let mut case = TestCase::new_server(
        "fixtures/request-chunked-echo.txt",
        "fixtures/response-chunked-echo-throttled.txt",
    )
    .await;
    case.throttle();
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
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

#[async_std::test]
async fn test_unexpected_eof() {
    // We can't predict unexpected EOF, so the response content-length is still 11
    let case = TestCase::new_server(
        "fixtures/request-unexpected-eof.txt",
        "fixtures/response-unexpected-eof.txt",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
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

#[async_std::test]
async fn test_invalid_trailer() {
    let case = TestCase::new_server(
        "fixtures/request-invalid-trailer.txt",
        "fixtures/response-invalid-trailer.txt",
    )
    .await;
    let addr = "http://example.com";

    async_h1::accept(addr, case.clone(), |req| async {
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
    .unwrap_err();

    assert!(case.read_result().await.is_empty());
}

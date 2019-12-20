mod common;
use async_h1::server;
use async_std::task;
use common::TestCase;
use http_types::{Response, StatusCode};

#[test]
fn test_basic_request() {
    task::block_on(async {
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
    });
}

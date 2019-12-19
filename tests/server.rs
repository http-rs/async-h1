mod common;
use async_h1::server;
use async_std::task;
use common::read_fixture;
use http_types::{Response, StatusCode};

#[test]
fn test_basic_request() {
    task::block_on(async {
        let request = read_fixture("request1").await;
        let mut expected = read_fixture("response1").await;
        let mut actual = Vec::new();
        let addr = "http://example.com";

        assert!(
            actual,
            expected,
            server::accept(addr, request.clone(), &mut actual, |_req| {
                async {
                    let mut resp = Response::new(StatusCode::Ok);
                    resp.set_body("");
                    Ok(resp)
                }
           })
        );
    })
}

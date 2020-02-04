use crate::common::fixture_path;
use async_h1::client;
use async_std::fs::File;
use async_std::io::SeekFrom;
use async_std::prelude::*;
use http_types::{headers, Method, Request, StatusCode};
use url::Url;

mod common;

use common::TestCase;

#[async_std::test]
async fn test_encode_request_add_date() {
    let case = TestCase::new_client("fixtures/request1.txt", "fixtures/response1.txt").await;

    let url = Url::parse("http://localhost:8080").unwrap();
    let mut req = Request::new(Method::Post, url);
    req.set_body("hello");

    let res = client::connect(case.clone(), req).await.unwrap();
    assert_eq!(res.status(), StatusCode::Ok);

    case.assert().await;
}

#[async_std::test]
async fn test_response_no_date() {
    let mut response_fixture = File::open(fixture_path("fixtures/response-no-date.txt"))
        .await
        .unwrap();
    response_fixture.seek(SeekFrom::Start(0)).await.unwrap();

    let res = client::decode(response_fixture).await.unwrap();

    pretty_assertions::assert_eq!(res.header(&headers::DATE).is_some(), true);
}

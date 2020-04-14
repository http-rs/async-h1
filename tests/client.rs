use crate::common::fixture_path;
use async_h1::client;
use async_std::fs::File;
use http_types::{headers, Method, Request, StatusCode};
use url::Url;

mod common;

use common::TestCase;

#[async_std::test]
async fn test_encode_request_add_date() {
    let case =
        TestCase::new_client("fixtures/add-date-req.http", "fixtures/add-date-res.http").await;

    let url = Url::parse("http://localhost:8080").unwrap();
    let mut req = Request::new(Method::Post, url);
    req.set_body("hello");

    let res = client::connect(case.clone(), req).await.unwrap();
    assert_eq!(res.status(), StatusCode::Ok);

    case.assert().await;
}

#[async_std::test]
async fn test_response_no_date() {
    let response_fixture = File::open(fixture_path("fixtures/no-date-res.http"))
        .await
        .unwrap();

    let res = client::decode(response_fixture).await.unwrap();

    pretty_assertions::assert_eq!(res.header(&headers::DATE).is_some(), true);
}

#[async_std::test]
async fn test_multiple_header_values_for_same_header_name() {
    let response_fixture = File::open(fixture_path("fixtures/multiple-cookies-res.http"))
        .await
        .unwrap();

    let res = client::decode(response_fixture).await.unwrap();

    pretty_assertions::assert_eq!(res.header(&headers::SET_COOKIE).unwrap().len(), 2);
}

#[async_std::test]
async fn test_response_newlines() {
    let response_fixture = File::open(fixture_path("fixtures/newlines-res.http"))
        .await
        .unwrap();

    let res = client::decode(response_fixture).await.unwrap();

    pretty_assertions::assert_eq!(
        res.header(&headers::CONTENT_LENGTH)
            .unwrap()
            .last()
            .unwrap()
            .as_str()
            .parse::<usize>()
            .unwrap(),
        78
    );
}

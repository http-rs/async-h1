mod client_decode {
    use async_h1::client;
    use async_std::io::Cursor;
    use http_types::headers;
    use http_types::Response;
    use http_types::Result;
    use pretty_assertions::assert_eq;

    async fn decode_lines(s: Vec<&str>) -> async_h1::Result<Option<Response>> {
        client::decode(Cursor::new(s.join("\r\n"))).await
    }

    #[async_std::test]
    async fn response_no_date() -> Result<()> {
        let res = decode_lines(vec![
            "HTTP/1.1 200 OK",
            "transfer-encoding: chunked",
            "content-type: text/plain",
            "",
            "",
        ])
        .await?
        .unwrap();

        assert_eq!(res.header(&headers::DATE).is_some(), true);
        Ok(())
    }

    #[async_std::test]
    async fn multiple_header_values_for_same_header_name() -> Result<()> {
        let res = decode_lines(vec![
            "HTTP/1.1 200 OK",
            "host: example.com",
            "content-length: 0",
            "set-cookie: sessionId=e8bb43229de9",
            "set-cookie: qwerty=219ffwef9w0f",
            "",
            "",
        ])
        .await?
        .unwrap();
        assert_eq!(res.header(&headers::SET_COOKIE).unwrap().iter().count(), 2);

        Ok(())
    }

    #[async_std::test]
    async fn response_newlines() -> Result<()> {
        let res = decode_lines(vec![
            "HTTP/1.1 200 OK",
            "content-length: 78",
            "date: {DATE}",
            "content-type: text/plain; charset=utf-8",
            "",
            "http specifies headers are separated with \r\n but many servers don't do that",
            "",
        ])
        .await?
        .unwrap();

        assert_eq!(res[headers::CONTENT_LENGTH], "78");

        Ok(())
    }
}

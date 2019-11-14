use async_h1::{server, Body};
use async_std::task;
use http::Response;
use std::{fs::File, io, path::PathBuf};

#[test]
fn test_keep_alive() {
    let request = read_fixture("request1");
    let mut actual = Vec::new();
    let request: &[u8] = &request;
    let body: &[u8] = "".as_bytes();

    let expected = read_fixture("response1");
    let expected: &[u8] = &expected;

    task::block_on(async {
        server::connect(request, &mut actual, |req| {
            async { Ok(Response::new(Body::empty(body))) }
        })
        .await
        .unwrap();
        assert!(actual == expected);
    });
}

fn read_fixture(name: &str) -> Vec<u8> {
    use std::io::Read;

    let directory: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let path: PathBuf = format!("tests/fixtures/{}.txt", name).into();
    let mut file = File::open(directory.join(path)).expect("Reading fixture file didn't work");
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .expect("Couldn't read fixture files contents");

    let mut result = Vec::<u8>::new();
    for byte in contents {
        if byte == 0x0A {
            result.push(0x0D);
        }
        result.push(byte);
    }
    result
}

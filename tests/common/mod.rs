use std::{fs::File, path::PathBuf};

#[macro_export]
macro_rules! assert {
    ($actual:expr, $expected:expr, $block:expr) => {
        task::block_on(async {
            $block.await.unwrap();
            pretty_assertions::assert_eq!(
                std::str::from_utf8(&$actual).unwrap(),
                std::str::from_utf8(&$expected).unwrap()
            );
        })
    };
}

pub fn read_fixture(name: &str) -> Vec<u8> {
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

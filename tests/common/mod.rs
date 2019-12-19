use async_std::io::Read;
use async_std::fs::File;
use async_std::path::PathBuf;

#[macro_export]
macro_rules! assert {
    ($actual:expr, $expected:expr, $block:expr) => {
        task::block_on(async {
            $block.await.unwrap();
            let mut actual = std::string::String::from_utf8($actual).unwrap();
            let mut expected = std::string::String::from_utf8($expected).unwrap();
            match expected.find("{DATE}") {
                Some(i) => {
                    expected.replace_range(i..i + 6, "");
                    match expected.get(i..i + 1) {
                        Some(byte) => {
                            let j = actual[i..].find(byte).expect("Byte not found");
                            actual.replace_range(i..i + j, "");
                        }
                        None => expected.replace_range(i.., ""),
                    }
                }
                None => {}
            }
            pretty_assertions::assert_eq!(actual, expected);
        })
    };
}

pub fn read_fixture(name: &str) -> Arc<File> {
    let directory: PathBuf = env!("CARGO_MANIFEST_DIR").into();
    let path: PathBuf = format!("tests/fixtures/{}.txt", name).into();
    let mut file = File::open(directory.join(path)).expect("Reading fixture file didn't work");
    Arc::new(file)
}

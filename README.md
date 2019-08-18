# async-h1
[![crates.io version][1]][2] [![build status][3]][4]
[![downloads][5]][6] [![docs.rs docs][7]][8]

Asynchronous HTTP 1.1 parser.

- [Documentation][8]
- [Crates.io][2]
- [Releases][releases]

## Examples
```rust
// tbi
```

## Installation
```sh
$ cargo add async-h1
```

## Safety
This crate uses ``#![deny(unsafe_code)]`` to ensure everything is implemented in
100% Safe Rust.

## Contributing
Want to join us? Check out our ["Contributing" guide][contributing] and take a
look at some of these issues:

- [Issues labeled "good first issue"][good-first-issue]
- [Issues labeled "help wanted"][help-wanted]

## Acknowledgements
This crate wouldn't have been possible without the excellent work done in
[hyper](https://github.com/hyperium/hyper/blob/b342c38f08972fe8be4ef9844e30f1e7a121bbc4/src/proto/h1/role.rs)

## License
[MIT](./LICENSE-MIT) OR [Apache-2.0](./LICENSE-APACHE)

[1]: https://img.shields.io/crates/v/async-h1.svg?style=flat-square
[2]: https://crates.io/crates/async-h1
[3]: https://img.shields.io/travis/rustasync/async-h1/master.svg?style=flat-square
[4]: https://travis-ci.org/rustasync/async-h1
[5]: https://img.shields.io/crates/d/async-h1.svg?style=flat-square
[6]: https://crates.io/crates/async-h1
[7]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[8]: https://docs.rs/async-h1

[releases]: https://github.com/rustasync/async-h1/releases
[contributing]: https://github.com/rustasync/async-h1/blob/master.github/CONTRIBUTING.md
[good-first-issue]: https://github.com/rustasync/async-h1/labels/good%20first%20issue
[help-wanted]: https://github.com/rustasync/async-h1/labels/help%20wanted

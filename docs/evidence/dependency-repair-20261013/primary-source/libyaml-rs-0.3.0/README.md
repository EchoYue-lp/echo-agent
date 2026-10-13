libyaml-rs
==========

[<img alt="github" src="https://img.shields.io/badge/github-yaml/libyaml--rs-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/yaml/libyaml-rs)
[<img alt="crates.io" src="https://img.shields.io/crates/v/libyaml-rs.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/libyaml-rs)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-libyaml--rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://docs.rs/libyaml-rs)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/yaml/libyaml-rs/ci.yml?branch=master&style=for-the-badge" height="20">](https://github.com/yaml/libyaml-rs/actions?query=branch%3Amaster)

This library is [libyaml] translated from C to unsafe Rust with the assistance
of [c2rust].
This repo is maintained by the [YAML organization](https://github.com/yaml) as
a fork of the archived [dtolnay/unsafe-libyaml](
https://github.com/dtolnay/unsafe-libyaml).

[libyaml]: https://github.com/yaml/libyaml/tree/2c891fc7a770e8ba2fec34fc6b545c672beb37e6
[c2rust]: https://github.com/immunant/c2rust

```toml
[dependencies]
libyaml-rs = "0.3"
```

*Compiler support: requires rustc 1.60+*

## License

<a href="LICENSE-MIT">MIT license</a>, same as libyaml.

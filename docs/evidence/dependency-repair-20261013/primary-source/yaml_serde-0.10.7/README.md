YAML Serde
==========

[<img alt="github" src="https://img.shields.io/badge/github-yaml/yaml--serde-8da0cb?style=for-the-badge&labelColor=555555&logo=github" height="20">](https://github.com/yaml/yaml-serde)
[<img alt="crates.io" src="https://img.shields.io/crates/v/yaml_serde.svg?style=for-the-badge&color=fc8d62&logo=rust" height="20">](https://crates.io/crates/yaml_serde)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-yaml__serde-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs" height="20">](https://docs.rs/yaml_serde)
[<img alt="build status" src="https://img.shields.io/github/actions/workflow/status/yaml/yaml-serde/ci.yml?branch=main&style=for-the-badge" height="20">](https://github.com/yaml/yaml-serde/actions?query=branch%3Amain)

> **This is the actively maintained fork of
> [serde-yaml](https://github.com/dtolnay/serde-yaml), published as `yaml_serde`
> by the official [YAML organization](https://github.com/yaml).**
>
> The original `serde_yaml` crate is no longer maintained.
> This fork continues development with full compatibility.
>
> Original author: David Tolnay

Rust library for using the [Serde] serialization framework with data in [YAML]
file format.

[Serde]: https://github.com/serde-rs/serde
[YAML]: https://yaml.org/

## Dependency

```toml
[dependencies]
serde = "1.0"
yaml_serde = "0.10"
```

Release notes are available under [GitHub releases].

[GitHub releases]: https://github.com/yaml/yaml-serde/releases

## Migrating from `serde_yaml`

To migrate with minimal code changes, use Cargo's package renaming:

```toml
[dependencies]
serde_yaml = { package = "yaml_serde", version = "0.10" }
```

This lets you keep all your existing `use serde_yaml::` imports unchanged.

Alternatively, update your imports directly:

```toml
[dependencies]
yaml_serde = "0.10"
```

Then change `use serde_yaml::` to `use yaml_serde::` in your code.

## Using Serde YAML

[API documentation is available in rustdoc form][docs.rs] but the general idea
is:

[docs.rs]: https://docs.rs/yaml_serde

```rust
use std::collections::BTreeMap;

fn main() -> Result<(), yaml_serde::Error> {
    // You have some type.
    let mut map = BTreeMap::new();
    map.insert("x".to_string(), 1.0);
    map.insert("y".to_string(), 2.0);

    // Serialize it to a YAML string.
    let yaml = yaml_serde::to_string(&map)?;
    assert_eq!(yaml, "x: 1.0\ny: 2.0\n");

    // Deserialize it back to a Rust type.
    let deserialized_map: BTreeMap<String, f64> = yaml_serde::from_str(&yaml)?;
    assert_eq!(map, deserialized_map);
    Ok(())
}
```

It can also be used with Serde's derive macros to handle structs and enums
defined in your program.

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
yaml_serde = "0.10"
```

Structs serialize in the obvious way:

```rust
use serde::{Serialize, Deserialize};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Point {
    x: f64,
    y: f64,
}

fn main() -> Result<(), yaml_serde::Error> {
    let point = Point { x: 1.0, y: 2.0 };

    let yaml = yaml_serde::to_string(&point)?;
    assert_eq!(yaml, "x: 1.0\ny: 2.0\n");

    let deserialized_point: Point = yaml_serde::from_str(&yaml)?;
    assert_eq!(point, deserialized_point);
    Ok(())
}
```

Enums serialize using YAML's `!tag` syntax to identify the variant name.

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
enum Enum {
    Unit,
    Newtype(usize),
    Tuple(usize, usize, usize),
    Struct { x: f64, y: f64 },
}

fn main() -> Result<(), yaml_serde::Error> {
    let yaml = "
        - !Newtype 1
        - !Tuple [0, 0, 0]
        - !Struct {x: 1.0, y: 2.0}
    ";
    let values: Vec<Enum> = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(values[0], Enum::Newtype(1));
    assert_eq!(values[1], Enum::Tuple(0, 0, 0));
    assert_eq!(values[2], Enum::Struct { x: 1.0, y: 2.0 });

    // The last two in YAML's block style instead:
    let yaml = "
        - !Tuple
          - 0
          - 0
          - 0
        - !Struct
          x: 1.0
          y: 2.0
    ";
    let values: Vec<Enum> = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(values[0], Enum::Tuple(0, 0, 0));
    assert_eq!(values[1], Enum::Struct { x: 1.0, y: 2.0 });

    // Variants with no data can be written using !Tag or just the string name.
    let yaml = "
        - Unit  # serialization produces this one
        - !Unit
    ";
    let values: Vec<Enum> = yaml_serde::from_str(yaml).unwrap();
    assert_eq!(values[0], Enum::Unit);
    assert_eq!(values[1], Enum::Unit);

    Ok(())
}
```

<br>

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>

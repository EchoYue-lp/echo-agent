# serde_yaml2

This crate provides serde integration for [yaml-rust2](https://github.com/Ethiraric/yaml-rust2/)


## Usage

See [examples](examples) dir for usage examples


## Enum representation

Enums are represented using external tagging. For example:

````rust
#[derive(Serialize, Debug, PartialEq)]
enum TestEnum {
    VariantA,
    VariantB(i32, f64),
}
````

will be represented as:

````
VariantA: ~
````

or 

````
VariantB: [1, 4.5]
````

## Running tests

Just execute 

````
cargo test
````

//! trybuild: applying #[derive(Extract)] to an enum must produce compile_error!

use stygian_extract_derive::Extract;

#[derive(Extract)]
enum MyEnum {
    Variant,
}

fn main() {}

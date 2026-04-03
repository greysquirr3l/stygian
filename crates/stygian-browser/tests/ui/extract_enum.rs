//! trybuild: applying #[derive(Extract)] to an enum must produce compile_error!

use stygian_browser::extract::Extract;

#[derive(Extract)]
enum MyEnum {
    Variant,
}

fn main() {}

//! trybuild: a struct field missing #[selector(...)] must produce compile_error!

use stygian_extract_derive::Extract;

#[derive(Extract)]
struct MyStruct {
    name: String,
}

fn main() {}

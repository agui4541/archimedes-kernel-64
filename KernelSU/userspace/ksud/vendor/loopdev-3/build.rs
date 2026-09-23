use bindgen::Builder;
use std::{env::var, path::PathBuf};

fn main() {
    let bindings = Builder::default()
        .rust_target(
            env!("CARGO_PKG_RUST_VERSION")
                .parse()
                .expect("valid rust version"),
        )
        .header_contents("wrapper.h", "#include <linux/loop.h>")
        .derive_default(true)
        .generate()
        .expect("Could not generate bindings");

    let mut bindings_path = PathBuf::from(var("OUT_DIR").unwrap());
    bindings_path.push("bindings.rs");
    bindings
        .write_to_file(&bindings_path)
        .expect("Could not write bindings to file");
}

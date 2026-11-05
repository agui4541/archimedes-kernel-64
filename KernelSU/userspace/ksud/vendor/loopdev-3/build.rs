use bindgen::Builder;
use std::{env::var, path::PathBuf};

fn main() {
    let mut builder = Builder::default()
        .rust_target(
            env!("CARGO_PKG_RUST_VERSION")
                .parse()
                .expect("valid rust version"),
        )
        .header_contents("wrapper.h", "#include <linux/loop.h>")
        .derive_default(true);

    // Debian/Ubuntu keep x86 UAPI asm headers under a multiarch include
    // directory. Without this, bindgen cannot resolve linux/loop.h on the
    // Linux build host even though the headers are installed.
    if std::path::Path::new("/usr/include/x86_64-linux-gnu").is_dir() {
        builder = builder.clang_arg("-I/usr/include/x86_64-linux-gnu");
    }

    let bindings = builder.generate()
        .expect("Could not generate bindings");

    let mut bindings_path = PathBuf::from(var("OUT_DIR").unwrap());
    bindings_path.push("bindings.rs");
    bindings
        .write_to_file(&bindings_path)
        .expect("Could not write bindings to file");
}

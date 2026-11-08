use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;

fn get_git_version() -> Result<(u32, String), std::io::Error> {
    // The kernel side of this Archimedes tree is pinned to KernelSU 11872.
    // The upstream git-count scheme produced 10310 for this checkout, which
    // makes ZygiskNext reject an otherwise compatible userspace daemon. Keep
    // the userspace code aligned with the kernel, while allowing CI or a
    // downstream build to override it explicitly.
    let version_code = match env::var("KSU_VERSION_CODE") {
        Ok(value) => value.trim().parse().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to parse KSU_VERSION_CODE",
            )
        })?,
        Err(_) => 11872,
    };

    let version_name = String::from_utf8(
        Command::new("git")
            .args(["describe", "--tags", "--always"])
            .output()?
            .stdout,
    )
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to read git describe stdout",
        )
    })?;
    let version_name = version_name.trim_start_matches('v').to_string();
    Ok((version_code, version_name))
}

fn main() {
    let (code, name) = match get_git_version() {
        Ok((code, name)) => (code, name),
        Err(_) => {
            // show warning if git is not installed
            println!("cargo:warning=Failed to get git version, using 0.0.0");
            (0, "0.0.0".to_string())
        }
    };
    let out_dir = env::var("OUT_DIR").expect("Failed to get $OUT_DIR");
    let out_dir = Path::new(&out_dir);
    File::create(Path::new(out_dir).join("VERSION_CODE"))
        .expect("Failed to create VERSION_CODE")
        .write_all(code.to_string().as_bytes())
        .expect("Failed to write VERSION_CODE");

    File::create(Path::new(out_dir).join("VERSION_NAME"))
        .expect("Failed to create VERSION_NAME")
        .write_all(name.trim().as_bytes())
        .expect("Failed to write VERSION_NAME");
}

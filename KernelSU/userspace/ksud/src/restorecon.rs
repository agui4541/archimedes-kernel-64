use crate::defs;
use anyhow::{Context, Result};
use jwalk::{Parallelism, WalkDir};
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "android"))]
use extattr::{lsetxattr, Flags as XattrFlags};

pub const SYSTEM_CON: &str = "u:object_r:system_file:s0";
pub const ADB_CON: &str = "u:object_r:adb_data_file:s0";
pub const UNLABEL_CON: &str = "u:object_r:unlabeled:s0";

const SELINUX_XATTR: &str = "security.selinux";

pub fn lsetfilecon<P: AsRef<Path>>(path: P, con: &str) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    lsetxattr(&path, SELINUX_XATTR, con, XattrFlags::empty()).with_context(|| {
        format!(
            "Failed to change SELinux context for {}",
            path.as_ref().display()
        )
    })?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn lgetfilecon<P: AsRef<Path>>(path: P) -> Result<String> {
    let con = extattr::lgetxattr(&path, SELINUX_XATTR).with_context(|| {
        format!(
            "Failed to get SELinux context for {}",
            path.as_ref().display()
        )
    })?;
    Ok(String::from_utf8_lossy(&con).to_string())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn setsyscon<P: AsRef<Path>>(path: P) -> Result<()> {
    lsetfilecon(path, SYSTEM_CON)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn setsyscon<P: AsRef<Path>>(_path: P) -> Result<()> {
    Ok(())
}

pub fn restore_syscon<P: AsRef<Path>>(dir: P) -> Result<()> {
    set_tree_context(dir.as_ref(), SYSTEM_CON)
}

fn set_tree_context(root: &Path, context: &str) -> Result<()> {
    for entry in WalkDir::new(root).parallelism(Parallelism::Serial) {
        if let Some(path) = entry.ok().map(|entry| entry.path()) {
            lsetfilecon(&path, context)?;
        }
    }
    Ok(())
}

fn restore_module_contexts() -> Result<()> {
    let modules_root = Path::new(defs::MODULE_DIR);
    if !modules_root.exists() {
        return Ok(());
    }

    // The module image itself and module metadata/scripts stay adb_data_file.
    // Only files that are later bound into a partition receive that
    // partition's file type.
    lsetfilecon(modules_root, ADB_CON)?;
    let contexts = [
        ("system", SYSTEM_CON),
        ("vendor", "u:object_r:vendor_file:s0"),
        ("product", "u:object_r:product_file:s0"),
        ("system_ext", "u:object_r:system_ext_file:s0"),
        ("odm", "u:object_r:odm_file:s0"),
        ("oem", "u:object_r:oemfs_file:s0"),
    ];

    for module_entry in std::fs::read_dir(modules_root)? {
        let module_root = match module_entry {
            Ok(entry) if entry.path().is_dir() => entry.path(),
            Ok(_) => continue,
            Err(error) => {
                log::warn!("read module entry failed: {error:#}");
                continue;
            }
        };
        lsetfilecon(&module_root, ADB_CON)?;
        for entry in std::fs::read_dir(&module_root)? {
            let entry = entry?;
            let path = entry.path();
            let context = contexts
                .iter()
                .find(|(partition, _)| entry.file_name() == *partition)
                .map(|(_, context)| *context)
                .unwrap_or(ADB_CON);
            set_tree_context(&path, context)?;
        }
    }
    Ok(())
}

pub fn restorecon() -> Result<()> {
    lsetfilecon(defs::DAEMON_PATH, ADB_CON)?;
    restore_module_contexts()?;
    Ok(())
}

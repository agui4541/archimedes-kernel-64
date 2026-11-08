use crate::defs;
use anyhow::Result;
use jwalk::{Parallelism::Serial, WalkDir};
use std::path::Path;
#[cfg(target_os = "android")]
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "android"))]
use anyhow::{Context, Ok};
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
    let con = String::from_utf8_lossy(&con);
    Ok(con.to_string())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn setsyscon<P: AsRef<Path>>(path: P) -> Result<()> {
    lsetfilecon(path, SYSTEM_CON)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn setsyscon<P: AsRef<Path>>(path: P) -> Result<()> {
    unimplemented!()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn lgetfilecon<P: AsRef<Path>>(path: P) -> Result<String> {
    unimplemented!()
}

pub fn restore_syscon<P: AsRef<Path>>(dir: P) -> Result<()> {
    for dir_entry in WalkDir::new(dir).parallelism(Serial) {
        if let Some(path) = dir_entry.ok().map(|dir_entry| dir_entry.path()) {
            setsyscon(&path)?;
        }
    }
    Ok(())
}

fn restore_syscon_if_unlabeled<P: AsRef<Path>>(dir: P) -> Result<()> {
    for dir_entry in WalkDir::new(dir).parallelism(Serial) {
        if let Some(path) = dir_entry.ok().map(|dir_entry| dir_entry.path()) {
            if let anyhow::Result::Ok(con) = lgetfilecon(&path) {
                if con == UNLABEL_CON || con.is_empty() {
                    lsetfilecon(&path, SYSTEM_CON)?;
                }
            }
        }
    }
    Ok(())
}

fn restore_module_partition_contexts() -> Result<()> {
    // Modules are stored below /data/adb/modules/<module-id>/<partition>/.
    // Walk every enabled module instead of looking for a nonexistent
    // /data/adb/modules/<partition> directory.
    let contexts = [
        ("system", SYSTEM_CON),
        ("vendor", "u:object_r:vendor_file:s0"),
        ("product", "u:object_r:product_file:s0"),
        ("system_ext", "u:object_r:system_ext_file:s0"),
        ("odm", "u:object_r:odm_file:s0"),
        ("oem", "u:object_r:oemfs_file:s0"),
    ];
    let modules_root = Path::new(defs::MODULE_DIR);
    let entries = match std::fs::read_dir(modules_root) {
        std::result::Result::Ok(entries) => entries,
        Err(error) => {
            log::warn!("read module root {} failed: {error:#}", modules_root.display());
            return Ok(());
        }
    };
    for module_entry in entries {
        let module_root = match module_entry {
            std::result::Result::Ok(entry) if entry.path().is_dir() => entry.path(),
            std::result::Result::Ok(_) => continue,
            Err(error) => {
                log::warn!("read module entry failed: {error:#}");
                continue;
            }
        };
        for (partition, context) in contexts {
            let root = module_root.join(partition);
            if !root.exists() {
                continue;
            }
            let mut needs_chcon = false;
            for entry in WalkDir::new(&root).parallelism(Serial) {
                if let Some(path) = entry.ok().map(|entry| entry.path()) {
                    if let Err(error) = lsetfilecon(&path, context) {
                        needs_chcon = true;
                        log::warn!(
                            "set module context {} -> {} failed: {error:#}",
                            path.display(),
                            context
                        );
                    } else if let std::result::Result::Ok(actual) = lgetfilecon(&path) {
                        if actual != context {
                            needs_chcon = true;
                        }
                    } else {
                        needs_chcon = true;
                    }
                } else {
                    needs_chcon = true;
                }
            }
            // Some Android 9 ext4 images accept the xattr write but keep
            // lower inodes unlabeled when the image was created in recovery.
            // Let the platform chcon applet repair the whole partition tree.
            #[cfg(target_os = "android")]
            if needs_chcon {
                match Command::new("/system/bin/chcon")
                    .args(["-R", context])
                    .arg(&root)
                    .status()
                {
                    std::result::Result::Ok(status) if status.success() => {}
                    std::result::Result::Ok(status) => log::warn!(
                        "chcon module tree {} -> {} exited with {}",
                        root.display(),
                        context,
                        status
                    ),
                    std::result::Result::Err(error) => log::warn!(
                        "chcon module tree {} -> {} failed: {error:#}",
                        root.display(),
                        context
                    ),
                }
            }
        }
    }
    Ok(())
}

pub fn restorecon() -> Result<()> {
    lsetfilecon(defs::DAEMON_PATH, ADB_CON)?;
    restore_module_partition_contexts()?;
    restore_syscon_if_unlabeled(defs::MODULE_DIR)?;
    Ok(())
}

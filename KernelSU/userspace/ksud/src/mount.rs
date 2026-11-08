use anyhow::{anyhow, Ok, Result};

#[cfg(any(target_os = "linux", target_os = "android"))]
use anyhow::Context;
#[cfg(any(target_os = "linux", target_os = "android"))]
use rustix::{fd::AsFd, fs::CWD, mount::*};

use crate::defs::KSU_OVERLAY_SOURCE;
use log::{info, warn};
#[cfg(any(target_os = "linux", target_os = "android"))]
use procfs::process::Process;
use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub struct AutoMountExt4 {
    target: String,
    auto_umount: bool,
}

impl AutoMountExt4 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn try_new(source: &str, target: &str, auto_umount: bool) -> Result<Self> {
        mount_ext4(source, target)?;
        Ok(Self {
            target: target.to_string(),
            auto_umount,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn try_new(_src: &str, _mnt: &str, _auto_umount: bool) -> Result<Self> {
        unimplemented!()
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn umount(&self) -> Result<()> {
        unmount(self.target.as_str(), UnmountFlags::DETACH)?;
        Ok(())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
impl Drop for AutoMountExt4 {
    fn drop(&mut self) {
        log::info!(
            "AutoMountExt4 drop: {}, auto_umount: {}",
            self.target,
            self.auto_umount
        );
        if self.auto_umount {
            let _ = self.umount();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn mount_debug(message: impl AsRef<str>) {
    let path = Path::new("/data/adb/ksu/log/overlay-mount-debug.log");
    if let std::result::Result::Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", message.as_ref());
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn mountpoint_in_mountinfo(target: &Path, mountinfo: &str) -> bool {
    let wanted = target.to_string_lossy().trim_end_matches('/').to_string();
    mountinfo.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .map(|mountpoint| mountpoint.trim_end_matches('/'))
            == Some(wanted.as_str())
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn mountpoint_is_present(target: &Path) -> bool {
    let self_mounted = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|contents| mountpoint_in_mountinfo(target, &contents))
        .unwrap_or(false);
    if self_mounted {
        return true;
    }

    // ksud may be launched from a transient init action namespace while the
    // Android mount helper operates in PID 1's namespace.  Check init's
    // mountinfo before deciding that the helper failed.
    std::fs::read_to_string("/proc/1/mountinfo")
        .map(|contents| mountpoint_in_mountinfo(target, &contents))
        .unwrap_or(false)
}

fn capture_kernel_dmesg(tag: &str) {
    let path = format!("/cache/ksu-dmesg-{tag}.log");
    if let std::result::Result::Ok(mut file) = File::create(&path) {
        let _ = writeln!(file, "capture-start");
        for command in ["/system/bin/dmesg", "/sbin/dmesg", "dmesg"] {
            match std::process::Command::new(command).output() {
                std::result::Result::Ok(output) if output.status.success() => {
                    let _ = file.write_all(&output.stdout);
                    let _ = file.flush();
                    return;
                }
                std::result::Result::Ok(output) => {
                    let _ = writeln!(file, "{} exit={}", command, output.status);
                }
                std::result::Result::Err(error) => {
                    let _ = writeln!(file, "{} error={error:#}", command);
                }
            }
        }
    }
}

pub fn mount_ext4(source: impl AsRef<Path>, target: impl AsRef<Path>) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        // The Android 9 toybox mount applet does not accept an image file as
        // a source on this build (it falls through to /etc/fstab). Attach the
        // image explicitly with losetup, then mount the resulting block node.
        // This keeps loop setup in the platform tools while ksud still owns
        // the transaction and works from the init-triggered namespace.
        let loop_output = std::process::Command::new("/system/bin/losetup")
            .args(["-f", "-s"])
            .arg(source.as_ref())
            .output()
            .with_context(|| format!("Failed to exec losetup for {}", source.as_ref().display()))?;
        let loop_stderr = String::from_utf8_lossy(&loop_output.stderr);
        let loop_device = String::from_utf8_lossy(&loop_output.stdout).trim().to_string();
        if loop_output.status.success() && !loop_device.is_empty() {
            let mount_output = std::process::Command::new("/system/bin/mount")
                .args(["-t", "ext4"])
                .arg(&loop_device)
                .arg(target.as_ref())
                .output()
                .with_context(|| format!("Failed to exec mount for {}", loop_device))?;
            let mount_stderr = String::from_utf8_lossy(&mount_output.stderr);
            if mount_output.status.success() || mountpoint_is_present(target.as_ref()) {
                return Ok(());
            }
            let _ = std::process::Command::new("/system/bin/losetup")
                .args(["-d", &loop_device])
                .status();
            log::warn!(
                "Android block mount failed for {} -> {}: {}",
                loop_device,
                target.as_ref().display(),
                mount_stderr.trim()
            );
        } else {
            log::warn!(
                "Android losetup failed for {}: {}",
                source.as_ref().display(),
                loop_stderr.trim()
            );
        }
    }

    let new_loopback = loopdev::LoopControl::open()?
        .next_free()
        .with_context(|| "Failed to alloc loop")?;
    new_loopback
        .with()
        .attach(source.as_ref())
        .with_context(|| "Failed to attach loop")?;
    let lo = new_loopback.path().ok_or(anyhow!("no loop"))?;
    if let Result::Ok(fs) = fsopen("ext4", FsOpenFlags::FSOPEN_CLOEXEC) {
        let fs = fs.as_fd();
        fsconfig_set_string(fs, "source", lo)?;
        fsconfig_create(fs)?;
        let mount = fsmount(fs, FsMountFlags::FSMOUNT_CLOEXEC, MountAttrFlags::empty())?;
        move_mount(
            mount.as_fd(),
            "",
            CWD,
            target.as_ref(),
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )?;
    } else {
        mount(lo, target.as_ref(), "ext4", MountFlags::empty(), "")?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn umount_dir(src: impl AsRef<Path>) -> Result<()> {
    unmount(src.as_ref(), UnmountFlags::empty())
        .with_context(|| format!("Failed to umount {}", src.as_ref().display()))?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn overlay_context_for(dest: &Path) -> Option<&'static str> {
    let path = dest.to_string_lossy();
    if path == "/system" || path.starts_with("/system/") {
        Some("u:object_r:system_file:s0")
    } else if path == "/vendor" || path.starts_with("/vendor/") {
        Some("u:object_r:vendor_file:s0")
    } else if path == "/product" || path.starts_with("/product/") {
        Some("u:object_r:product_file:s0")
    } else if path == "/system_ext" || path.starts_with("/system_ext/") {
        Some("u:object_r:system_ext_file:s0")
    } else if path == "/odm" || path.starts_with("/odm/") {
        Some("u:object_r:odm_file:s0")
    } else if path == "/oem" || path.starts_with("/oem/") {
        Some("u:object_r:oemfs_file:s0")
    } else {
        None
    }
}

pub fn mount_overlayfs(
    lower_dirs: &[String],
    lowest: &str,
    upperdir: Option<PathBuf>,
    workdir: Option<PathBuf>,
    dest: impl AsRef<Path>,
) -> Result<()> {
    let lowerdir_config = lower_dirs
        .iter()
        .map(|s| s.as_ref())
        .chain(std::iter::once(lowest))
        .collect::<Vec<_>>()
        .join(":");
    info!(
        "mount overlayfs on {:?}, lowerdir={}, upperdir={:?}, workdir={:?}",
        dest.as_ref(),
        lowerdir_config,
        upperdir,
        workdir
    );

    let upperdir = upperdir
        .filter(|up| up.exists())
        .map(|e| e.display().to_string());
    let workdir = workdir
        .filter(|wd| wd.exists())
        .map(|e| e.display().to_string());

    mount_debug(format!("begin dest={} lower={} upper={:?} work={:?}", dest.as_ref().display(), lowerdir_config, upperdir, workdir));
    let result = (|| {
        let fs = fsopen("overlay", FsOpenFlags::FSOPEN_CLOEXEC)?;
        let fs = fs.as_fd();
        fsconfig_set_string(fs, "lowerdir", &lowerdir_config)?;
        if let (Some(upperdir), Some(workdir)) = (&upperdir, &workdir) {
            fsconfig_set_string(fs, "upperdir", upperdir)?;
            fsconfig_set_string(fs, "workdir", workdir)?;
        }
        if let Some(context) = overlay_context_for(dest.as_ref()) {
            fsconfig_set_string(fs, "context", context)?;
        }
        fsconfig_set_string(fs, "source", KSU_OVERLAY_SOURCE)?;
        fsconfig_create(fs)?;
        let mount = fsmount(fs, FsMountFlags::FSMOUNT_CLOEXEC, MountAttrFlags::empty())?;
        move_mount(
            mount.as_fd(),
            "",
            CWD,
            dest.as_ref(),
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )
    })();

    if let Err(e) = result {
        mount_debug(format!("fsopen failed: {e:#}"));
        warn!("fsopen mount failed: {:#}, fallback to mount", e);
        let mut data = format!("lowerdir={lowerdir_config},override_creds=off");
        if let Some(context) = overlay_context_for(dest.as_ref()) {
            data = format!("{data},context={context}");
        }
        if let (Some(upperdir), Some(workdir)) = (upperdir, workdir) {
            data = format!("{data},upperdir={upperdir},workdir={workdir}");
        }
        let legacy = mount(
            KSU_OVERLAY_SOURCE,
            dest.as_ref(),
            "overlay",
            MountFlags::empty(),
            data,
        );
        match &legacy {
            std::result::Result::Ok(()) => mount_debug("legacy mount succeeded"),
            Err(err) => mount_debug(format!("legacy mount failed: {err:#}")),
        }
        legacy?;
    } else {
        mount_debug("fsopen mount succeeded");
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn mount_tmpfs(dest: impl AsRef<Path>) -> Result<()> {
    info!("mount tmpfs on {}", dest.as_ref().display());
    if let Result::Ok(fs) = fsopen("tmpfs", FsOpenFlags::FSOPEN_CLOEXEC) {
        let fs = fs.as_fd();
        fsconfig_set_string(fs, "source", KSU_OVERLAY_SOURCE)?;
        fsconfig_create(fs)?;
        let mount = fsmount(fs, FsMountFlags::FSMOUNT_CLOEXEC, MountAttrFlags::empty())?;
        move_mount(
            mount.as_fd(),
            "",
            CWD,
            dest.as_ref(),
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )?;
    } else {
        mount(
            KSU_OVERLAY_SOURCE,
            dest.as_ref(),
            "tmpfs",
            MountFlags::empty(),
            "",
        )?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn bind_mount(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    info!(
        "bind mount {} -> {}",
        from.as_ref().display(),
        to.as_ref().display()
    );
    if let Result::Ok(tree) = open_tree(
        CWD,
        from.as_ref(),
        OpenTreeFlags::OPEN_TREE_CLOEXEC
            | OpenTreeFlags::OPEN_TREE_CLONE
            | OpenTreeFlags::AT_RECURSIVE,
    ) {
        move_mount(
            tree.as_fd(),
            "",
            CWD,
            to.as_ref(),
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )?;
    } else {
        let flags = if from.as_ref().is_dir() {
            MountFlags::BIND | MountFlags::REC
        } else {
            MountFlags::BIND
        };
        mount(from.as_ref(), to.as_ref(), "", flags, "")?;
    }
    Ok(())
}


#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn bind_mount_shallow(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    info!(
        "shallow bind mount {} -> {}",
        from.as_ref().display(),
        to.as_ref().display()
    );
    // A root lower mirror must not clone child mounts. They remain attached
    // below the destination mount and are visible through the overlay, while
    // the private mirror prevents the new overlay from propagating back into
    // its own lower tree.
    if let std::result::Result::Ok(tree) = open_tree(
        CWD,
        from.as_ref(),
        OpenTreeFlags::OPEN_TREE_CLOEXEC | OpenTreeFlags::OPEN_TREE_CLONE,
    ) {
        move_mount(
            tree.as_fd(),
            "",
            CWD,
            to.as_ref(),
            MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH,
        )?;
    } else {
        mount(from.as_ref(), to.as_ref(), "", MountFlags::BIND, "")?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn make_mount_slave(path: impl AsRef<Path>) -> Result<()> {
    let target = CString::new(path.as_ref().as_os_str().as_encoded_bytes())?;
    let empty = CString::new("")?;
    let ret = unsafe {
        libc::mount(
            empty.as_ptr(),
            target.as_ptr(),
            empty.as_ptr(),
            (libc::MS_SLAVE | libc::MS_REC) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn move_mount_path(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
    let source = CString::new(from.as_ref().as_os_str().as_encoded_bytes())?;
    let target = CString::new(to.as_ref().as_os_str().as_encoded_bytes())?;
    let empty = CString::new("")?;
    let ret = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            empty.as_ptr(),
            libc::MS_MOVE as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn make_mount_private(path: impl AsRef<Path>) -> Result<()> {
    // /system is a shared mount on this Android 9 tree. A bind clone used as
    // an overlay lowerdir must not receive the overlay mount propagated back
    // into it, otherwise the lowerdir becomes the overlay itself.
    let target = CString::new(path.as_ref().as_os_str().as_encoded_bytes())?;
    let empty = CString::new("")?;
    let ret = unsafe {
        libc::mount(
            empty.as_ptr(),
            target.as_ptr(),
            empty.as_ptr(),
            (libc::MS_PRIVATE | libc::MS_REC) as libc::c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn mount_overlay_child(
    mount_point: &str,
    relative: &String,
    module_roots: &Vec<String>,
    stock_root: &String,
) -> Result<()> {
    if !module_roots
        .iter()
        .any(|lower| Path::new(&format!("{lower}{relative}")).exists())
    {
        return bind_mount(stock_root, mount_point);
    }
    if !Path::new(&stock_root).is_dir() {
        return Ok(());
    }
    let mut lower_dirs: Vec<String> = vec![];
    for lower in module_roots {
        let lower_dir = format!("{lower}{relative}");
        let path = Path::new(&lower_dir);
        if path.is_dir() {
            lower_dirs.push(lower_dir);
        } else if path.exists() {
            // stock root has been blocked by this file
            return Ok(());
        }
    }
    if lower_dirs.is_empty() {
        return Ok(());
    }
    // merge modules and stock
    if let Err(e) = mount_overlayfs(&lower_dirs, stock_root, None, None, mount_point) {
        warn!("failed: {:#}, fallback to bind mount", e);
        bind_mount(stock_root, mount_point)?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn snapshot_child_mounts(root: &str, mount_seq: &[String]) -> Result<Vec<(String, PathBuf)>> {
    let mirror_name = root.trim_start_matches('/').replace('/', "_");
    let mirror_root = Path::new("/data/adb/ksu/overlay-mirrors").join(mirror_name);
    let _ = std::fs::remove_dir_all(&mirror_root);
    if std::fs::create_dir_all(&mirror_root).is_err() {
        mount_debug(format!("cannot create child mirror root {}", mirror_root.display()));
        return Err(anyhow!("cannot create child mirror root {}", mirror_root.display()));
    }

    let mut mirrors: Vec<(String, PathBuf)> = Vec::new();
    for (index, mount_point) in mount_seq.iter().enumerate() {
        let source = Path::new(mount_point);
        let mirror = mirror_root.join(index.to_string());
        let prepare = if source.is_dir() {
            std::fs::create_dir_all(&mirror)
        } else {
            {
                let parent_result: std::io::Result<()> = match mirror.parent() {
                    Some(parent) => std::fs::create_dir_all(parent),
                    None => std::result::Result::Ok(()),
                };
                parent_result.and_then(|_| File::create(&mirror).map(|_| ()))
            }
        };
        if let Err(e) = prepare {
            mount_debug(format!("child mirror prepare failed path={} error={e:#}", mount_point));
            for (_, previous) in mirrors.iter().rev() {
                let _ = umount_dir(previous);
            }
            return Err(anyhow!("child mirror prepare failed for {mount_point}: {e:#}"));
        }
        match bind_mount(source, &mirror).and_then(|_| make_mount_private(&mirror)) {
            std::result::Result::Ok(()) => mirrors.push((mount_point.clone(), mirror)),
            Err(e) => {
                mount_debug(format!("child mirror bind failed path={} error={e:#}", mount_point));
                let _ = umount_dir(&mirror);
                for (_, previous) in mirrors.iter().rev() {
                    let _ = umount_dir(previous);
                }
                return Err(anyhow!("child mirror bind failed for {mount_point}: {e:#}"));
            }
        }
    }
    Ok(mirrors)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn mount_overlay(
    root: &String,
    module_roots: &Vec<String>,
    workdir: Option<PathBuf>,
    upperdir: Option<PathBuf>,
) -> Result<()> {
    info!("mount overlay for {}", root);
    std::env::set_current_dir(root).with_context(|| format!("failed to chdir to {root}"))?;
    // Keep the target mount propagation unchanged. The lower mirror is
    // made private below, so the overlay cannot propagate back into it.
    // The stock root must not be used as its own lowerdir on old 4.9 kernels.
    // Keep a recursive bind in the data namespace and overlay onto the original.
    let root_mirror = Path::new("/data/adb/ksu/overlay-mirrors").join(
        root.trim_start_matches('/').replace('/', "_") + "_root",
    );
    let _ = std::fs::remove_dir_all(&root_mirror);
    std::fs::create_dir_all(&root_mirror)
        .with_context(|| format!("create root mirror {}", root_mirror.display()))?;
    if let Err(e) = bind_mount_shallow(root, &root_mirror) {
        return Err(anyhow!("root mirror bind failed for {root}: {e:#}"));
    }
    make_mount_private(&root_mirror)
        .with_context(|| format!("make root mirror private for {root}"))?;
    // The recursive root mirror retains the original child mounts in the
    // lower tree. Do not detach and reattach them: on this device that
    // sequence returns EPERM after the root overlay is installed.
    let stock_root = root_mirror.display().to_string();
    if let Err(error) = mount_overlayfs(module_roots, &stock_root, upperdir, workdir, root) {
        mount_debug(format!("root overlay mount failed error={error:#}"));
        let _ = umount_dir(&root_mirror);
        return Err(error).with_context(|| "mount overlayfs for root failed");
    }
    mount_debug(format!("root overlay installed with recursive lower root={}", root));
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn mount_ext4(_src: &str, _target: &str, _autodrop: bool) -> Result<()> {
    unimplemented!()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn umount_dir(_src: &str) -> Result<()> {
    unimplemented!()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn mount_overlay(
    _root: &String,
    _module_roots: &Vec<String>,
    _workdir: Option<PathBuf>,
    _upperdir: Option<PathBuf>,
) -> Result<()> {
    unimplemented!()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn mount_tmpfs(_dest: impl AsRef<Path>) -> Result<()> {
    unimplemented!()
}

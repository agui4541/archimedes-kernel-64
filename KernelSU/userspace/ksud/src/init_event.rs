use anyhow::{Context, Result};
use log::{info, warn};
use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}};

use crate::module::prune_modules;
use crate::{
    assets, defs, ksucalls, mount, restorecon,
    utils::{self, ensure_clean_dir},
};

fn mount_partition(partition_name: &str, lowerdir: &Vec<String>) -> Result<()> {
    if lowerdir.is_empty() {
        warn!("partition: {partition_name} lowerdir is empty");
        return Ok(());
    }

    let partition = format!("/{partition_name}");

    // if /partition is a symlink and linked to /system/partition, then we don't need to overlay it separately
    if Path::new(&partition).read_link().is_ok() {
        warn!("partition: {partition} is a symlink");
        return Ok(());
    }

    let mut workdir = None;
    let mut upperdir = None;
    let system_rw_dir = Path::new(defs::SYSTEM_RW_DIR);
    if system_rw_dir.exists() {
        // A read-only overlay can replace the partition root, but Android 9's
        // 4.9 mount namespace rejects reattaching pre-existing child mounts
        // underneath it. Keep a writable upper/work pair on the module
        // filesystem so those child mounts can be rebound after the root
        // overlay is installed.
        let partition_rw_dir = system_rw_dir.join(partition_name);
        let work = partition_rw_dir.join("workdir");
        let upper = partition_rw_dir.join("upperdir");
        std::fs::create_dir_all(&work)
            .with_context(|| format!("create overlay workdir {}", work.display()))?;
        std::fs::create_dir_all(&upper)
            .with_context(|| format!("create overlay upperdir {}", upper.display()))?;
        // Overlay upper/work directories live under adb_data_file. Match the
        // partition label before mounting so enforcing SELinux does not reject
        // writes or turn the overlay into an unlabeled tree.
        let stock = Path::new(&partition);
        if let Ok(context) = restorecon::lgetfilecon(stock) {
            for path in [&partition_rw_dir, &upper, &work] {
                if let Err(error) = restorecon::lsetfilecon(path, &context) {
                    warn!("set overlay context {} failed: {error:#}", path.display());
                }
            }
        }
        workdir = Some(work);
        upperdir = Some(upper);
    }

    mount::mount_overlay(&partition, lowerdir, workdir, upperdir)
}

fn has_overlay_ancestor(path: &Path) -> bool {
    let wanted = path.to_string_lossy().trim_end_matches('/').to_string();
    let Ok(contents) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    contents.lines().any(|line| {
        let Some(mountpoint) = line.split_whitespace().nth(4) else {
            return false;
        };
        let filesystem = line.split(" - ").nth(1).unwrap_or_default();
        if !filesystem.starts_with("overlay ") {
            return false;
        }
        let mountpoint = mountpoint
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\134", "\\")
            .trim_end_matches('/')
            .to_string();
        wanted == mountpoint || wanted.starts_with(&(mountpoint + "/"))
    })
}

fn bind_module_tree(source: &Path, target: &Path) -> Result<()> {
    let entries = std::fs::read_dir(source)
        .with_context(|| format!("read module tree {}", source.display()))?;
    for entry in entries.flatten() {
        let src = entry.path();
        let dst = target.join(entry.file_name());
        let metadata = match std::fs::symlink_metadata(&src) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // A successful directory overlay already exposes additions and keeps
        // the system_file/vendor_file superblock context. Binding the same
        // child again would replace it with the module image's unlabeled
        // source inode and recreate SELinux denials.
        if has_overlay_ancestor(&dst) {
            continue;
        }
        if metadata.is_dir() {
            if dst.is_dir() {
                bind_module_tree(&src, &dst)?;
            }
        } else if metadata.is_file() && dst.exists() {
            if let Err(e) = mount::bind_mount(&src, &dst) {
                warn!("bind module file {} -> {} failed: {:#}", src.display(), dst.display(), e);
            }
        }
    }
    Ok(())
}

fn bind_module_partition(module_dir: &str, partition: &str) -> Result<()> {
    let entries = std::fs::read_dir(module_dir)
        .with_context(|| format!("read module dir {}", module_dir))?;
    let target = Path::new("/").join(partition);
    for entry in entries.flatten() {
        let module = entry.path();
        // .rw/ stores overlay upper/work directories and is not a user
        // module. Treating it as one can create empty overlays on odm/product
        // and gives those mounts an unlabeled SELinux context.
        if module.file_name().and_then(|name| name.to_str()) == Some(".rw")
            || !module.is_dir()
            || module.join(defs::DISABLE_FILE_NAME).exists()
            || module.join(defs::SKIP_MOUNT_FILE_NAME).exists()
        {
            continue;
        }
        let source = module.join(partition);
        if source.is_dir() && target.is_dir() {
            bind_module_tree(&source, &target)?;
        }
    }
    Ok(())
}


fn mountpoint_has_descendant(path: &Path) -> bool {
    let wanted = path.to_string_lossy().trim_end_matches('/').to_string();
    let prefix = format!("{wanted}/");
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|contents| {
            contents.lines().any(|line| {
                let Some(mountpoint) = line.split_whitespace().nth(4) else {
                    return false;
                };
                let decoded = mountpoint
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\134", "\\");
                decoded == wanted || decoded.starts_with(&prefix)
            })
        })
        .unwrap_or(false)
}

fn collect_module_overlay_targets(
    source: &Path,
    partition_root: &Path,
    relative: &Path,
    targets: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(source) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            let next = relative.join(entry.file_name());
            collect_module_overlay_targets(&path, partition_root, &next, targets);
            continue;
        }

        // Find the nearest existing stock directory for this module file.
        // This lets an overlay expose a new module subdirectory without
        // replacing /system itself.
        let mut candidate = relative.to_path_buf();
        loop {
            if candidate.as_os_str().is_empty() {
                break;
            }
            let target = partition_root.join(&candidate);
            if target.is_dir() {
                targets.insert(target);
                break;
            }
            if !candidate.pop() {
                break;
            }
        }
    }
}

fn mount_module_partition_overlay_dirs(module_dir: &str, partition: &str) -> Result<()> {
    let partition_root = Path::new("/").join(partition);
    if !partition_root.is_dir() {
        return Ok(());
    }

    let mut targets = HashSet::new();
    let entries = std::fs::read_dir(module_dir)
        .with_context(|| format!("read module dir {}", module_dir))?;
    for entry in entries.flatten() {
        let module = entry.path();
        if module.file_name().and_then(|name| name.to_str()) == Some(".rw")
            || !module.is_dir()
            || module.join(defs::DISABLE_FILE_NAME).exists()
            || module.join(defs::SKIP_MOUNT_FILE_NAME).exists()
        {
            continue;
        }
        let source = module.join(partition);
        if source.is_dir() {
            collect_module_overlay_targets(&source, &partition_root, Path::new(""), &mut targets);
        }
    }

    // Deepest targets first. Once a child overlay is installed, a parent with
    // a mounted descendant is skipped; this avoids nesting over GSI child
    // mounts such as /system/etc/permissions/*.
    let mut targets: Vec<PathBuf> = targets.into_iter().collect();
    targets.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for target in targets {
        if target == partition_root || mountpoint_has_descendant(&target) {
            warn!("skip overlay target with mounted child: {}", target.display());
            continue;
        }
        let relative = match target.strip_prefix(&partition_root) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let mut lower_dirs = Vec::new();
        let entries = std::fs::read_dir(module_dir)
            .with_context(|| format!("read module dir {}", module_dir))?;
        for entry in entries.flatten() {
            let module = entry.path();
            if module.file_name().and_then(|name| name.to_str()) == Some(".rw")
                || !module.is_dir()
                || module.join(defs::DISABLE_FILE_NAME).exists()
                || module.join(defs::SKIP_MOUNT_FILE_NAME).exists()
            {
                continue;
            }
            let source = module.join(partition).join(relative);
            if source.is_dir() {
                lower_dirs.push(source.display().to_string());
            }
        }
        if lower_dirs.is_empty() {
            continue;
        }
        let Some(stock) = target.to_str() else {
            continue;
        };
        match mount::mount_overlayfs(&lower_dirs, stock, None, None, &target) {
            Ok(()) => info!("directory overlay installed on {}", target.display()),
            Err(error) => warn!(
                "directory overlay failed on {}: {error:#}; bind fallback will cover existing files",
                target.display()
            ),
        }
    }
    Ok(())
}

fn enter_overlay_mount_context() {
    let current = std::fs::read_to_string("/proc/self/attr/current")
        .unwrap_or_default()
        .trim()
        .to_string();
    if current == "u:r:init:s0" {
        match std::fs::write("/proc/self/attr/current", "u:r:su:s0") {
            Ok(()) => info!("overlay mount context switched init -> su"),
            Err(error) => warn!("overlay mount context switch failed: {error:#}"),
        }
    } else {
        info!("overlay mount context={current}");
    }
}

pub fn mount_modules_systemlessly(module_dir: &str) -> Result<()> {
    // Init launches ksud in the init SELinux domain. Switch to KernelSU's
    // permissive domain before walking an unlabeled ext4 modules image;
    // otherwise restorecon sees an empty tree and the overlay lowerdir keeps
    // the unlabeled context.
    enter_overlay_mount_context();
    // OverlayFS is opt-in on this legacy Android 9 device. A stale or partially
    // installed module must never create a root overlay during early boot.
    let overlay_requested = Path::new("/data/adb/ksu/overlay.enable").exists();
    let system_root_image = utils::getprop("ro.build.system_root_image")
        .as_deref()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let force_overlay = Path::new("/data/adb/ksu/overlay.force").exists();
    let gsi_overlay_opt_in = Path::new("/data/adb/ksu/overlay.gsi").exists();
    let late_overlay = Path::new("/data/adb/ksu/overlay.late").exists();
    // A late overlay is enabled only from on_boot_completed. A stale
    // handoff marker must never make the early mount path replace /system.
    let late_overlay_active = Path::new("/data/adb/ksu/overlay.late.active").exists()
        && utils::getprop("sys.boot_completed").as_deref() == Some("1");
    let is_phh_gsi = utils::getprop("ro.product.device")
        .as_deref()
        .map(|v| v.starts_with("phhgsi") || v.contains("phh"))
        .unwrap_or(false);
    // phh/GSI system mounts are replaced late by the framework on this
    // Android 9 device. Root overlayfs is therefore opt-in there; ordinary
    // KernelSU modules keep the proven per-file bind behavior until the user
    // explicitly creates overlay.gsi after a clean baseline is verified.
    let overlay_enabled = overlay_requested
        && (!system_root_image || force_overlay)
        && (!is_phh_gsi || gsi_overlay_opt_in)
        && (!late_overlay || late_overlay_active);
    if overlay_requested && is_phh_gsi && !gsi_overlay_opt_in {
        warn!("phh GSI detected; using bind mounts (create overlay.gsi to opt in)");
    } else if overlay_requested && system_root_image && !force_overlay {
        warn!("system-as-root detected; using legacy bind mounts (create overlay.force to opt in)");
    } else if overlay_requested && late_overlay && !late_overlay_active {
        warn!("late overlay requested; keep early boot on bind mounts");
    }
    if !overlay_enabled {
        info!("overlayfs not enabled; using bind fallback");
        for partition in ["system", "vendor", "product", "system_ext", "odm", "oem"] {
            let _ = bind_module_partition(module_dir, partition);
        }
        return Ok(());
    }
    // The phh/GSI mount graph contains shared child mounts below /system.
    // Replacing the root hides or invalidates /system/bin/sh and causes the
    // exact early shell failure seen in the root-overlay experiment. Use
    // directory overlays only where no child mount is underneath, then keep
    // per-file binds for the remaining paths.
    if is_phh_gsi {
        info!("phh GSI: using safe directory overlays plus bind fallback");
        for partition in ["system", "vendor", "product", "system_ext", "odm", "oem"] {
            if let Err(error) = mount_module_partition_overlay_dirs(module_dir, partition) {
                warn!("directory overlay scan failed for {partition}: {error:#}");
            }
            if let Err(error) = bind_module_partition(module_dir, partition) {
                warn!("bind fallback failed for {partition}: {error:#}");
            }
        }
        return Ok(());
    }

    let dir = std::fs::read_dir(module_dir)
        .with_context(|| format!("open {} failed", defs::MODULE_DIR))?;
    let partitions = ["vendor", "product", "system_ext", "odm", "oem"];
    let mut system_lowerdir: Vec<String> = Vec::new();
    let mut partition_lowerdir: HashMap<String, Vec<String>> = HashMap::new();
    for partition in partitions {
        partition_lowerdir.insert(partition.to_string(), Vec::new());
    }

    for entry in dir.flatten() {
        let module = entry.path();
        // .rw/ stores overlay upper/work directories and is not a user
        // module. Treating it as one can create empty overlays on odm/product
        // and gives those mounts an unlabeled SELinux context.
        if module.file_name().and_then(|name| name.to_str()) == Some(".rw")
            || !module.is_dir()
            || module.join(defs::DISABLE_FILE_NAME).exists()
            || module.join(defs::SKIP_MOUNT_FILE_NAME).exists()
        {
            continue;
        }
        let module_system = module.join("system");
        if module_system.is_dir() {
            system_lowerdir.push(module_system.display().to_string());
        }
        for partition in partitions {
            let part_path = module.join(partition);
            if part_path.is_dir() {
                partition_lowerdir
                    .get_mut(partition)
                    .expect("partition initialized")
                    .push(part_path.display().to_string());
            }
        }
    }

    // OverlayFS is the primary path: it preserves additions and directory
    // merges. The per-file bind path is only a compatibility fallback for
    // a partition whose mount is rejected by the kernel or SELinux policy.
    if !system_lowerdir.is_empty() {
        if let Err(e) = mount_partition("system", &system_lowerdir) {
            warn!("overlay mount system failed: {:#}; using bind fallback", e);
            bind_module_partition(module_dir, "system")?;
        }
    }
    for partition in partitions {
        let lowerdir = partition_lowerdir
            .get(partition)
            .expect("partition initialized");
        if lowerdir.is_empty() {
            continue;
        }
        if let Err(e) = mount_partition(partition, lowerdir) {
            warn!("overlay mount {partition} failed: {:#}; using bind fallback", e);
            bind_module_partition(module_dir, partition)?;
        }
    }
    Ok(())
}

fn write_phase(phase: &str) {
    let _ = std::fs::write("/cache/ksu-post-fs.phase", phase);
}

fn is_mountpoint(path: &str) -> bool {
    let wanted = path.trim_end_matches('/');
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|contents| {
            contents.lines().any(|line| {
                line.split_whitespace()
                    .nth(4)
                    .map(|mountpoint| mountpoint.trim_end_matches('/'))
                    == Some(wanted)
            })
        })
        .unwrap_or(false)
}

fn is_overlay_mountpoint(path: &str) -> bool {
    std::fs::read_to_string("/proc/self/mountinfo")
        .map(|contents| {
            contents.lines().any(|line| {
                let target = line.split_whitespace().nth(4);
                let filesystem = line.split(" - ").nth(1).unwrap_or_default();
                target == Some(path) && filesystem.starts_with("overlay ")
            })
        })
        .unwrap_or(false)
}

fn mount_module_image_early() -> Result<bool> {
    let module_dir = defs::MODULE_DIR;
    if is_mountpoint(module_dir) {
        return Ok(true);
    }
    let module_img = Path::new(defs::MODULE_IMG);
    if !module_img.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(module_dir)
        .with_context(|| format!("create module directory {module_dir}"))?;
    if let Err(error) = restorecon::lsetfilecon(module_img, restorecon::ADB_CON) {
        warn!("set modules image context failed: {error:#}");
    }
    info!("early mount module image: {} to {}", module_img.display(), module_dir);
    mount::AutoMountExt4::try_new(defs::MODULE_IMG, module_dir, false)
        .with_context(|| "early mount module image failed")?;
    ksucalls::report_module_mounted();
    Ok(true)
}

fn mount_early_impl(run_post: bool) -> Result<()> {
    let _ = std::fs::write(
        "/cache/ksu-pre-mount.log",
        format!("entered pre-mount pid={}\n", std::process::id()),
    );
    // The `on fs` action can run just before userdata is exposed on this GSI.
    // Wait briefly for the opt-in marker; otherwise post-fs-data remains the
    // safe fallback for normal boots.
    let overlay_marker = Path::new("/data/adb/ksu/overlay.enable");
    for _ in 0..100 {
        if overlay_marker.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !overlay_marker.exists() {
        info!("pre-mount: overlay marker not available");
        return if run_post { on_post_data_fs() } else { Ok(()) };
    }
    let marker = Path::new(defs::WORKING_DIR).join(defs::PRE_MOUNT_FILE_NAME);
    let _ = std::fs::remove_file(&marker);
    if utils::is_safe_mode() {
        warn!("safe mode, skip early overlay mount");
        return Ok(());
    }
    utils::umask(0);
    if let Err(error) = utils::switch_mnt_ns(1) {
        warn!("switch to init mount namespace failed in pre-mount: {error:#}");
    }
    enter_overlay_mount_context();
    assets::ensure_binaries(true).with_context(|| "early overlay binaries")?;
    if !mount_module_image_early()? {
        warn!("early overlay has no modules image");
        return Ok(());
    }
    if let Err(e) = restorecon::restorecon() {
        warn!("early restorecon failed: {e:#}");
    }
    if crate::module::load_sepolicy_rule().is_err() {
        warn!("early load sepolicy.rule failed");
    }
    if let Err(e) = mount_modules_systemlessly(defs::MODULE_DIR) {
        warn!("early systemless mount failed: {e:#}");
        return Err(e);
    }
    let _ = std::fs::write(
        "/cache/ksu-early-ns.txt",
        std::fs::read_link("/proc/self/ns/mnt")
            .map(|v| format!("self_ns={v:?}\n"))
            .unwrap_or_default(),
    );
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-early-mountinfo", mountinfo);
    }
    std::fs::File::create(&marker)
        .with_context(|| format!("create early mount marker {}", marker.display()))?;
    info!("early overlay mount completed");
    if run_post { on_post_data_fs() } else { Ok(()) }
}

pub fn on_pre_mount() -> Result<()> {
    let marker = Path::new(defs::WORKING_DIR).join(defs::PRE_MOUNT_FILE_NAME);
    if marker.exists() {
        if is_mountpoint(defs::MODULE_DIR) {
            return on_post_data_fs();
        }
        // A previous boot may have died after writing the marker. Do not
        // treat that stale marker as proof that modules.img is mounted.
        let _ = std::fs::remove_file(&marker);
    }
    mount_early_impl(true)
}

pub fn on_pre_mount_worker() -> Result<()> {
    mount_early_impl(false)
}

pub fn on_post_data_fs() -> Result<()> {
    // The init event must mutate the init mount namespace. Some GSI paths
    // invoke ksud from a child namespace; switching explicitly avoids
    // mounting modules/overlay where only the helper process can see them.
    if let Err(error) = utils::switch_mnt_ns(1) {
        warn!("post-fs-data: switch to init mount namespace failed: {error:#}");
    }
    enter_overlay_mount_context();
    write_phase("post:start");
    ksucalls::report_post_fs_data();
    write_phase("post:reported");

    utils::umask(0);
    let post_fs_done = Path::new(defs::WORKING_DIR).join("post-fs-data.done");
    let boot_completed = utils::getprop("sys.boot_completed").as_deref() == Some("1");
    if boot_completed && post_fs_done.exists() {
        info!("post-fs-data already completed for this boot");
        return Ok(());
    }
    let pre_mount_done = Path::new(defs::WORKING_DIR).join(defs::PRE_MOUNT_FILE_NAME);
    if !boot_completed {
        let _ = std::fs::remove_file(&post_fs_done);
    }
    let safe_mode = crate::utils::is_safe_mode();
    // pre-mount and post-fs-data run in the same ksud process now. The
    // marker is written only after modules.img and systemless mounts succeed;
    // re-probing mountinfo here can see a different namespace and re-enter the
    // image cleanup path, which deadlocks the legacy 4.9 mount code. A stale
    // marker from a failed boot is not evidence of a live mount.
    let module_dir_mounted = is_mountpoint(defs::MODULE_DIR);
    let early_mounted = pre_mount_done.exists() && module_dir_mounted;
    if !early_mounted {
        let _ = std::fs::remove_file(&pre_mount_done);
    }
    let bootguard_file = Path::new(defs::WORKING_DIR).join(defs::BOOTGUARD_FILE_NAME);
    // The GSI path has no dependable boot-completed callback, so a
    // persistent bootguard marker would survive into the next boot and put
    // ksud in safe mode forever. Keep bootguard for the stock product only.
    let bootguard_enabled = !utils::getprop("ro.product.device")
        .as_deref()
        .map(|value| value.starts_with("phhgsi") || value.contains("phh"))
        .unwrap_or(false)
        && utils::getprop("ro.product.vendor.device")
            .as_deref()
            .map(|value| value == "Archimedes")
            .unwrap_or(false);
    if bootguard_enabled && !safe_mode {
        if let Err(e) = std::fs::File::create(&bootguard_file) {
            warn!("create bootguard marker failed: {e:#}");
        } else {
            info!("bootguard marker armed");
        }
    }

    write_phase("post:before_bootlog");
    #[cfg(unix)]
    let _ = catch_bootlog("logcat", vec!["logcat"]);
    #[cfg(unix)]
    let _ = catch_bootlog("dmesg", vec!["dmesg", "-w"]);
    write_phase("post:after_bootlog");

    if utils::has_magisk() {
        warn!("Magisk detected, skip post-fs-data!");
        return Ok(());
    }


    if safe_mode {
        // we should still mount modules.img to `/data/adb/modules` in safe mode
        // becuase we may need to operate the module dir in safe mode
        warn!("safe mode, skip common post-fs-data.d scripts");
    } else {
        write_phase("post:before_common_scripts");
        // Then exec common post-fs-data scripts
        if let Err(e) = crate::module::exec_common_scripts("post-fs-data.d", true) {
            warn!("exec common post-fs-data scripts failed: {}", e);
        }
    }

    let module_update_img = defs::MODULE_UPDATE_IMG;
    let module_img = defs::MODULE_IMG;
    let module_dir = defs::MODULE_DIR;
    let module_update_flag = Path::new(defs::WORKING_DIR).join(defs::UPDATE_FILE_NAME);

    write_phase("post:before_image");
    if !module_dir_mounted {
        // modules.img is the default image. Never clean a directory that is
        // already a mounted modules image: doing so can return EBUSY and can
        // destroy the active module view on this legacy Android 9 GSI.
        let mut target_update_img = &module_img;
        ensure_clean_dir(module_dir)?;
        assets::ensure_binaries(true).with_context(|| "Failed to extract bin assets")?;

        if Path::new(module_update_img).exists() {
            if module_update_flag.exists() {
                target_update_img = &module_update_img;
            } else {
                std::fs::remove_file(module_update_img)?;
            }
        }

        if !Path::new(target_update_img).exists() {
            return Ok(());
        }

        if let Err(error) = restorecon::lsetfilecon(target_update_img, restorecon::ADB_CON) {
            warn!("set module image context failed: {error:#}");
        }
        info!("mount module image: {target_update_img} to {module_dir}");
        mount::AutoMountExt4::try_new(target_update_img, module_dir, false)
            .with_context(|| "mount module image failed".to_string())?;

        if target_update_img == &module_update_img {
            std::fs::remove_file(module_update_flag)?;
        }
        ksucalls::report_module_mounted();
    } else {
        info!("module image already mounted; keep the active module filesystem");
    }
    // The first restorecon pass can race the legacy loop mount on Android 9.
    // Retry after the image is known to be mounted, before overlay lowerdirs
    // are installed, so an unlabeled image is repaired in the same boot.
    if let Err(e) = restorecon::restorecon() {
        warn!("post-image restorecon failed: {e:#}");
    }
    write_phase("post:after_image");

    // if we are in safe mode, we should disable all modules
    if safe_mode {
        warn!("safe mode, skip post-fs-data scripts and disable all modules!");
        if let Err(e) = crate::module::disable_all_modules() {
            warn!("disable all modules failed: {}", e);
        }
        return Ok(());
    }

    if let Err(e) = prune_modules() {
        warn!("prune modules failed: {}", e);
    }

    if let Err(e) = restorecon::restorecon() {
        warn!("restorecon failed: {}", e);
    }

    // load sepolicy.rule
    if crate::module::load_sepolicy_rule().is_err() {
        warn!("load sepolicy.rule failed");
    }

    if let Err(e) = crate::profile::apply_sepolies() {
        warn!("apply root profile sepolicy failed: {}", e);
    }

    write_phase("post:before_tmpfs");
    // mount temp dir
    if let Err(e) = mount::mount_tmpfs(utils::get_tmp_path()) {
        warn!("do temp dir mount failed: {}", e);
    }

    write_phase("post:before_stage_scripts");
    // exec modules post-fs-data scripts
    // TODO: Add timeout
    if let Err(e) = crate::module::exec_stage_script("post-fs-data", true) {
        warn!("exec post-fs-data scripts failed: {}", e);
    }

    write_phase("post:before_system_prop");
    // load system.prop
    if let Err(e) = crate::module::load_system_prop() {
        warn!("load system.prop failed: {}", e);
    }

    write_phase("post:before_late_mount");
    if !early_mounted {
        if Path::new("/data/adb/ksu/overlay.enable").exists() {
        }
        if let Err(e) = mount_modules_systemlessly(module_dir) {
            warn!("do systemless mount failed: {}", e);
        }
    } else {
        info!("skip duplicate systemless mount after on-fs overlay");
    }
    let executable = std::env::current_exe().unwrap_or_else(|_| Path::new("/cache/ksud").to_path_buf());
    let _ = std::process::Command::new(executable).arg("mount-watch").spawn();
    write_phase("post:before_done");
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-post-mountinfo", mountinfo);
    }
    if let Err(e) = std::fs::File::create(&post_fs_done) {
        warn!("create post-fs-data completion marker failed: {e:#}");
    }

    run_stage("post-mount", true);

    std::env::set_current_dir("/").with_context(|| "failed to chdir to /")?;

    Ok(())
}

fn run_stage(stage: &str, block: bool) {
    utils::umask(0);

    if utils::has_magisk() {
        warn!("Magisk detected, skip {stage}");
        return;
    }

    if crate::utils::is_safe_mode() {
        warn!("safe mode, skip {stage} scripts");
        return;
    }

    if let Err(e) = crate::module::exec_common_scripts(&format!("{stage}.d"), block) {
        warn!("Failed to exec common {stage} scripts: {e}");
    }
    if let Err(e) = crate::module::exec_stage_script(stage, block) {
        warn!("Failed to exec {stage} scripts: {e}");
    }
}

pub fn mount_watch() -> Result<()> {
    use std::io::Write;
    if let Err(error) = utils::switch_mnt_ns(1) {
        log::warn!("mount-watch: switch to init mount namespace failed: {error:#}");
    }
    let path = "/cache/ksu-mount-watch.log";
    let mut file = std::fs::File::create(path)?;
    let mut last: Option<(bool, bool)> = None;
    for tick in 0..2400u32 {
        let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let has_overlay = info.lines().any(|line| line.contains(" - overlay ") && line.contains(" /system "));
        let has_modules = info.lines().any(|line| line.split_whitespace().nth(4) == Some("/data/adb/modules"));
        let state = (has_overlay, has_modules);
        if last != Some(state) {
            writeln!(file, "tick={} overlay={} modules={}", tick, has_overlay, has_modules)?;
            file.flush()?;
            last = Some(state);
        }
        // Do not remount from this watcher. On phh/GSI the system mount
        // graph is still settling around boot-completed; repeated overlay
        // attempts can race init and make /system/bin/sh unavailable. The
        // single boot-completed callback is the only reapply point.
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}

pub fn on_services() -> Result<()> {
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-services-before-mountinfo", mountinfo);
    }
    info!("on_services triggered!");
    run_stage("service", false);
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-services-after-mountinfo", mountinfo);
    }

    Ok(())
}

pub fn on_boot_completed() -> Result<()> {
    if let Err(error) = utils::switch_mnt_ns(1) {
        warn!("boot-completed: switch to init mount namespace failed: {error:#}");
    }
    // phh/GSI late mounts can replace /system after post-fs-data. Re-apply
    // the requested overlay after boot is complete, but only if the module
    // image is still mounted and /system is no longer an overlay.
    let overlay_enable = Path::new("/data/adb/ksu/overlay.enable");
    let overlay_late = Path::new("/data/adb/ksu/overlay.late");
    let overlay_late_active = Path::new("/data/adb/ksu/overlay.late.active");
    if overlay_enable.exists()
        && is_mountpoint(defs::MODULE_DIR)
        && !is_overlay_mountpoint("/system")
    {
        // On phh/GSI, /system has shared child mounts which are still being
        // assembled during post-fs-data. Keep the proven bind path until
        // boot-completed, then perform one overlay attempt in init's mount
        // namespace. The handoff marker is removed immediately so a crash
        // cannot make the next boot use the early overlay path.
        let late = overlay_late.exists();
        if late {
            if let Err(error) = std::fs::File::create(overlay_late_active) {
                warn!("create late overlay handoff failed: {error:#}");
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if let Err(error) = mount_modules_systemlessly(defs::MODULE_DIR) {
            warn!("boot-completed overlay reapply failed: {error:#}");
        } else {
            info!("boot-completed overlay reapply completed");
        }
        if late {
            let _ = std::fs::remove_file(overlay_late_active);
        }
    }
    let _ = std::fs::write(
        "/cache/ksu-boot-before-ns.txt",
        std::fs::read_link("/proc/self/ns/mnt")
            .map(|v| format!("self_ns={v:?}\n"))
            .unwrap_or_default(),
    );
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-boot-before-mountinfo", mountinfo);
    }
    ksucalls::report_boot_complete();
    // The marker is only an intra-boot handoff. Leaving it behind makes the
    // next boot skip the modules.img mount after an early mount failure.
    let pre_mount_done = Path::new(defs::WORKING_DIR).join(defs::PRE_MOUNT_FILE_NAME);
    let _ = std::fs::remove_file(pre_mount_done);
    info!("on_boot_completed triggered!");
    let bootguard_file = Path::new(defs::WORKING_DIR).join(defs::BOOTGUARD_FILE_NAME);
    if bootguard_file.exists() {
        // This callback is emitted after sys.boot_completed=1.  Clear the
        // marker in-process: GSI SELinux policy rejects executing both
        // /system/bin/sh and data-labeled helper binaries from init.
        if let Err(e) = std::fs::remove_file(&bootguard_file) {
            warn!("remove bootguard marker failed: {e:#}");
        } else {
            info!("bootguard marker cleared");
        }
        let watchdog_active = Path::new(defs::WORKING_DIR).join("overlay-watchdog.active");
        let watchdog_done = Path::new(defs::WORKING_DIR).join("overlay-watchdog.done");
        let _ = std::fs::remove_file(watchdog_active);
        let _ = std::fs::File::create(watchdog_done);
    }
    let module_update_img = Path::new(defs::MODULE_UPDATE_IMG);
    let module_img = Path::new(defs::MODULE_IMG);
    if module_update_img.exists() {
        // this is a update and we successfully booted
        if std::fs::rename(module_update_img, module_img).is_err() {
            warn!("Failed to rename images, copy it now.",);
            utils::copy_sparse_file(module_update_img, module_img, false)
                .with_context(|| "Failed to copy images")?;
            std::fs::remove_file(module_update_img).with_context(|| "Failed to remove image!")?;
        }
    }

    run_stage("boot-completed", false);
    if let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") {
        let _ = std::fs::write("/cache/ksu-boot-after-mountinfo", mountinfo);
    }

    Ok(())
}

#[cfg(unix)]
fn catch_bootlog(logname: &str, command: Vec<&str>) -> Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    let logdir = Path::new(defs::LOG_DIR);
    utils::ensure_dir_exists(logdir)?;
    let bootlog = logdir.join(format!("{logname}.log"));
    let oldbootlog = logdir.join(format!("{logname}.old.log"));

    if bootlog.exists() {
        std::fs::rename(&bootlog, oldbootlog)?;
    }

    let bootlog = std::fs::File::create(bootlog)?;

    let mut args = vec!["-s", "9", "30s"];
    args.extend_from_slice(&command);
    // timeout -s 9 30s logcat > boot.log
    let result = unsafe {
        std::process::Command::new("timeout")
            .process_group(0)
            .pre_exec(|| {
                utils::switch_cgroups();
                Ok(())
            })
            .args(args)
            .stdout(Stdio::from(bootlog))
            .spawn()
    };

    if let Err(e) = result {
        warn!("Failed to start logcat: {:#}", e);
    }

    Ok(())
}

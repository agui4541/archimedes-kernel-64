use anyhow::{Context, Result};
use log::{info, warn};
use std::{collections::HashMap, path::Path};

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
        workdir = Some(system_rw_dir.join(partition_name).join("workdir"));
        upperdir = Some(system_rw_dir.join(partition_name).join("upperdir"));
    }

    mount::mount_overlay(&partition, lowerdir, workdir, upperdir)
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
        if !module.is_dir()
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


fn arm_overlay_watchdog() {
    let active = Path::new(defs::WORKING_DIR).join("overlay-watchdog.active");
    let done = Path::new(defs::WORKING_DIR).join("overlay-watchdog.done");
    let _ = std::fs::remove_file(&done);
    if let Err(e) = std::fs::File::create(&active) {
        warn!("arm overlay watchdog marker failed: {e:#}");
        return;
    }
    let executable = std::env::current_exe().unwrap_or_else(|_| Path::new("/cache/ksud").to_path_buf());
    if let Err(e) = std::process::Command::new(executable)
        .arg("overlay-watchdog")
        .spawn()
    {
        warn!("start overlay watchdog failed: {e:#}");
    } else {
        info!("overlay watchdog armed");
    }
}

pub fn overlay_watchdog() -> Result<()> {
    std::thread::sleep(std::time::Duration::from_secs(90));
    let active = Path::new(defs::WORKING_DIR).join("overlay-watchdog.active");
    let done = Path::new(defs::WORKING_DIR).join("overlay-watchdog.done");
    if active.exists() && !done.exists() {
        warn!("overlay watchdog timeout; rebooting recovery");
        let reason = b"recovery\0";
        let result = unsafe {
            libc::syscall(
                libc::SYS_reboot,
                libc::LINUX_REBOOT_MAGIC1,
                libc::LINUX_REBOOT_MAGIC2,
                libc::LINUX_REBOOT_CMD_RESTART2,
                reason.as_ptr(),
            )
        };
        if result != 0 {
            warn!("direct recovery reboot failed: {}", std::io::Error::last_os_error());
            let _ = std::process::Command::new("/system/bin/reboot")
                .arg("recovery")
                .spawn();
        }
    }
    Ok(())
}
pub fn mount_modules_systemlessly(module_dir: &str) -> Result<()> {
    // OverlayFS is opt-in on this legacy Android 9 device. A stale or partially
    // installed module must never create a root overlay during early boot.
    let overlay_enabled = Path::new("/data/adb/ksu/overlay.enable").exists();
    if !overlay_enabled {
        info!("overlayfs not enabled; using bind fallback");
        for partition in ["system", "vendor", "product", "system_ext", "odm", "oem"] {
            let _ = bind_module_partition(module_dir, partition);
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
        if !module.is_dir()
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

pub fn on_post_data_fs() -> Result<()> {
    ksucalls::report_post_fs_data();

    utils::umask(0);
    let post_fs_done = Path::new(defs::WORKING_DIR).join("post-fs-data.done");
    let boot_completed = utils::getprop("sys.boot_completed").as_deref() == Some("1");
    if boot_completed && post_fs_done.exists() {
        info!("post-fs-data already completed for this boot");
        return Ok(());
    }
    if !boot_completed {
        let _ = std::fs::remove_file(&post_fs_done);
    }
    let safe_mode = crate::utils::is_safe_mode();
    if Path::new("/data/adb/ksu/overlay.enable").exists() {
        // Switch before starting log capture or module scripts. The vendor
        // 4.9 kernel rejects setns after the post-fs-data process has spawned
        // helper children.
        if let Err(error) = utils::switch_mnt_ns(1) {
            warn!("switch to init mount namespace failed, continuing: {error:#}");
        }
    }
    let bootguard_file = Path::new(defs::WORKING_DIR).join(defs::BOOTGUARD_FILE_NAME);
    let bootguard_enabled = utils::getprop("ro.product.vendor.device")
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

    #[cfg(unix)]
    let _ = catch_bootlog("logcat", vec!["logcat"]);
    #[cfg(unix)]
    let _ = catch_bootlog("dmesg", vec!["dmesg", "-w"]);

    if utils::has_magisk() {
        warn!("Magisk detected, skip post-fs-data!");
        return Ok(());
    }


    if safe_mode {
        // we should still mount modules.img to `/data/adb/modules` in safe mode
        // becuase we may need to operate the module dir in safe mode
        warn!("safe mode, skip common post-fs-data.d scripts");
    } else {
        // Then exec common post-fs-data scripts
        if let Err(e) = crate::module::exec_common_scripts("post-fs-data.d", true) {
            warn!("exec common post-fs-data scripts failed: {}", e);
        }
    }

    let module_update_img = defs::MODULE_UPDATE_IMG;
    let module_img = defs::MODULE_IMG;
    let module_dir = defs::MODULE_DIR;
    let module_update_flag = Path::new(defs::WORKING_DIR).join(defs::UPDATE_FILE_NAME);

    // modules.img is the default image
    let mut target_update_img = &module_img;

    // we should clean the module mount point if it exists
    ensure_clean_dir(module_dir)?;

    assets::ensure_binaries(true).with_context(|| "Failed to extract bin assets")?;

    if Path::new(module_update_img).exists() {
        if module_update_flag.exists() {
            // if modules_update.img exists, and the the flag indicate this is an update
            // this make sure that if the update failed, we will fallback to the old image
            // if we boot succeed, we will rename the modules_update.img to modules.img #on_boot_complete
            target_update_img = &module_update_img;
            // Keep the flag until the image is mounted successfully.  The
            // first post-fs-data event can run before /data is ready; if the
            // mount fails, the late retry must still treat this as an update.
        } else {
            // if modules_update.img exists, but the flag not exist, we should delete it
            std::fs::remove_file(module_update_img)?;
        }
    }

    if !Path::new(target_update_img).exists() {
        return Ok(());
    }

    // we should always mount the module.img to module dir
    // becuase we may need to operate the module dir in safe mode
    info!("mount module image: {target_update_img} to {module_dir}");
    mount::AutoMountExt4::try_new(target_update_img, module_dir, false)
        .with_context(|| "mount module image failed".to_string())?;

    if target_update_img == &module_update_img {
        std::fs::remove_file(module_update_flag)?;
    }

    // tell kernel that we've mount the module, so that it can do some optimization
    ksucalls::report_module_mounted();

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

    // mount temp dir
    if let Err(e) = mount::mount_tmpfs(utils::get_tmp_path()) {
        warn!("do temp dir mount failed: {}", e);
    }

    // exec modules post-fs-data scripts
    // TODO: Add timeout
    if let Err(e) = crate::module::exec_stage_script("post-fs-data", true) {
        warn!("exec post-fs-data scripts failed: {}", e);
    }

    // load system.prop
    if let Err(e) = crate::module::load_system_prop() {
        warn!("load system.prop failed: {}", e);
    }

    // Keep the standard early mount point: modules may need their files before
    // zygote starts. The separate health watcher only fixes marker cleanup.
    if Path::new("/data/adb/ksu/overlay.enable").exists() {
        // The in-process boot-complete callback clears the marker; only the
        // rescue watchdog is needed during the vulnerable early window.
        arm_overlay_watchdog();
    }
    if let Err(e) = mount_modules_systemlessly(module_dir) {
        warn!("do systemless mount failed: {}", e);
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

pub fn on_services() -> Result<()> {
    info!("on_services triggered!");
    run_stage("service", false);

    Ok(())
}

pub fn on_boot_completed() -> Result<()> {
    ksucalls::report_boot_complete();
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

# Archimedes reproducible build

The repository contains both the pure and KernelSU-compatible branches. The
build entry point is `build_archimedes.sh`; it creates a clean out-of-tree
build, uses the Archimedes defconfig, fixes the legacy DT binding lookup, and
defaults to 24 jobs. The kernel release contains `agui@caner.center`.

```sh
JOBS=24 ./build_archimedes.sh pure
JOBS=24 ./build_archimedes.sh ksu
```

The `ksu` variant enables `CONFIG_KSU` explicitly. KernelSU is pinned to the
legacy-compatible version used by this 4.9 tree; its version does not depend
on whether the source came from Git or a ZIP archive.

The known-good MTK v1 boot header and display DTB are checked in as
`tools/archimedes-boot-template-32MiB.img`. Package either build with:

```sh
python3 package_archimedes_boot.py \
  --template tools/archimedes-boot-template-32MiB.img \
  --kernel out-archimedes-pure/arch/arm64/boot/Image.gz-dtb \
  --output boot-archimedes-pure-32MiB.img
```

The packager requires `bootopt=64S3,32S1,64S1`, Android boot header v1,
2048-byte pages, the tested display DTB, and an exact 32 MiB output. It does
not alter partitions or reboot a device.

## KernelSU userspace pairing

The Archimedes boot image is an Android v1 image containing only the kernel and
known-good display DTB; it has no ramdisk. Therefore it cannot carry `ksud`.
A KernelSU boot must always be released together with the `ksud` binary built
from the same KernelSU source revision. Installing only the boot image leaves
whatever old `/cache/ksud` and `/data/adb/ksud` happen to be on the device.

In TWRP, install the matching userspace before booting Android:

```sh
adb push ksud /tmp/ksud
adb shell 'cp /tmp/ksud /cache/ksud; cp /tmp/ksud /data/adb/ksud; chmod 0755 /cache/ksud /data/adb/ksud; sync'
adb shell 'sha256sum /cache/ksud /data/adb/ksud'
```

Both hashes must match the release manifest. After boot, verify that
`/system/priv-app` is an `overlay` mount and that the KernelSU manager reports
`Working`; a module marker under the overlaid directory should be readable.
The current GSI path deliberately uses directory overlay mounts and bind
fallbacks because this 4.9 kernel returns `ENOSYS` for the newer `fsopen`
interface.
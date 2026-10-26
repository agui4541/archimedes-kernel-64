# Archimedes MT6761 64-bit kernel (KernelSU)

> **Status:** this repository builds the Archimedes Linux kernel with KernelSU. It is not a complete ROM or a one-click 32-bit-to-64-bit migration package. Read the release manifest and the limitations below before flashing.

Archimedes is a Xiaomi learning device based on MediaTek MT6761. The original retail software uses a 32-bit Android userspace. This project brings up an AArch64 4.9.117 kernel and documents a path toward a 64-bit GSI while retaining the device-specific vendor support.

## What is verified

The repository records a retail-device kernel baseline in [BASELINE-20260930.md](BASELINE-20260930.md). Both the pure and KernelSU kernel variants were reported boot-tested on that baseline; the KSU build is the focus of this branch. The record covers kernel boot and listed device functions. It does **not** certify every stock firmware, storage variant, GSI, or complete migration combination.

A previously tested retail boot combination was `boot + dtbo + lk + lk2 + vendor`. Treat that as a device-specific test result, not a universal image set: use only the matching files and partition map published together in a release. Never mix engineering, retail, or different-release images.

## Migration from stock 32-bit Android

Move in stages. Keep a known recovery path and a local copy of the device's own original images before changing partitions. Do not publish or share identity-bearing partitions such as NVRAM, NVDATA, protect, or calibration data.

### Stage 1: AArch64 kernel with the existing 32-bit userspace

1. Identify the exact device/storage variant and stock firmware build. This migration is currently documented for the **64 GB Archimedes** retail variant; the 16 GB variant is not validated for the same procedure.
2. Obtain one release bundle whose manifest names the source firmware, supported storage variant, every target partition, image SHA-256, flash method/order, and recovery procedure. If a bundle lacks that manifest, stop rather than guessing partition names or reusing images from another build.
3. From TWRP/recovery, flash only the release-matched partitions listed by that manifest. The previously tested retail combination is listed above. Read back and verify each written image before rebooting.
4. Keep the original 32-bit system at this stage. Confirm the device boots and that the release's stated functions work before moving to a GSI.

For the kernel build and 32 MiB boot-image packaging constraints, see [BUILD-ARCHIMEDES.md](BUILD-ARCHIMEDES.md) and [docs/boot-packaging-standard.md](docs/boot-packaging-standard.md). Packaging a boot image does not itself update any device partition.

### Stage 2: Optional 64-bit GSI userspace

A 64-bit kernel alone does not turn the installed Android userspace into 64-bit. A GSI migration additionally needs a compatible GSI, enough system space, matching vbmeta settings, and vendor components that work with the AArch64 kernel.

The checked-in [2.5 GiB expansion guide](docs/expand-system-2.5g.md) and script are **only for the documented Archimedes 64 GB layout**. Expansion recreates GPT entries and clears userdata; system/vbmeta/cache contents are also rebuilt. It is destructive and can make the device unbootable if the layout does not match. It is not supported for the 16 GB model. Make a device-specific recovery plan before running it.

After expansion, use a single tested package/manifest for `system`, `vendor`, `vbmeta`, and any additional partitions it explicitly requires. Do not assume that any image named “arm64 GSI” is compatible, or that a successful kernel boot proves the full GSI migration.

## Vendor compatibility

The retail vendor image contains 32-bit connectivity kernel modules that cannot be loaded by this AArch64 kernel; see [the connectivity notes](Documentation/archimedes-connectivity.md). This is a kernel-module ABI issue, not proof that every 32-bit vendor HAL must be converted. A 64-bit Android system can retain 32-bit vendor components only when its runtime and ABI configuration support them.

The preferred public solution is a **vendor compatibility patch generated from a known stock vendor image**, rather than a universal replacement vendor image:

- require an exact supported vendor build and verify its hash before patching;
- replace only the AArch64 kernel modules and the minimum explicitly tested configuration files;
- preserve file ownership, modes, symlinks, and SELinux labels;
- stop on an unknown vendor image instead of guessing;
- keep proprietary vendor blobs out of this source repository unless redistribution is authorized.

This hash-guarded patch workflow is a packaging direction, not a claim that a universal patcher is already available. Until a release publishes and validates one, use only its exact matched vendor image.

## Build this branch

The build helper creates an out-of-tree build and enables KernelSU for the `ksu` target:

```sh
JOBS=24 ./build_archimedes.sh ksu
```

See [BUILD-ARCHIMEDES.md](BUILD-ARCHIMEDES.md) for the toolchain, packaging command, and boot-image invariants. GitHub Actions builds the kernel boot artifact; it does not build a complete GSI ROM or a vendor image.

## Before reporting a boot failure

Use the image set and checksums from one release only. Record the firmware build and storage variant, preserve the first failure's logs before restoring a known-good image, and include the exact image hashes and partition names in the report. Follow [the boot packaging and logging standard](docs/boot-packaging-standard.md); a clean diagnostic log is more useful than a log captured after several unrelated boots.

## License

Kernel source is provided under GPL-2.0. See [COPYING](COPYING) for the full license text. Third-party driver sources and proprietary vendor files retain their own licenses and redistribution terms.

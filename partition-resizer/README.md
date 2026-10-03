# Generic partition resizer

`resize-partition.ps1` replaces the old Archimedes 16G/64G fixed-sector
scripts. It reads the live eMMC geometry through ADB while the device is in
TWRP, resolves partitions by name (or `pN`), and calculates all new starts and
ends from the current table. No capacity, partition number, or sector boundary
is hard-coded.

## Supported operation

- GPT on a physical eMMC (`/dev/block/mmcblkN`), with 512/4K logical sectors.
- Primary MBR is parsed and supported for four primary entries. Extended/EBR
  chains are refused (use GPT or a dedicated image backup for those).
- Physical (non-`super`) partitions only. Dynamic partitions must be resized
  with `lpmake` and are rejected.
- The target may grow or shrink. The donor is the later partition and is
  discarded/rewritten; fixed partitions between them are backed up on the PC,
  moved to the computed LBA, and restored.
- Automatic filesystem resize is ext4-only. `-SkipFilesystemResize` is for a
  target that will immediately be replaced by a newly flashed image.

This is a capability matrix, not a claim that an arbitrary filesystem can be
shrunk safely. Unsupported tables, filesystems, mounted partitions, unknown
devices, and unsafe geometry stop before the first write.

## Use

1. Boot TWRP and unmount System, Vendor, Product, Data, Cache and Metadata.
2. Run a dry plan (no device writes):

   ```powershell
   .\partition-resizer\resize-partition.ps1 `
     -Partition system -SizeMiB 2560 -Donor userdata
   ```

3. Review `partition-backup-*/plan.json`. For an actual operation, repeat with
   `-Apply`; type a lower-case `y` at the final prompt. The donor contents are
   deliberately not backed up, so treat this as a data-wiping operation.

   ```powershell
   .\partition-resizer\resize-partition.ps1 `
     -Partition system -SizeMiB 2560 -Donor userdata -Apply
   ```

4. Keep the generated directory. It contains the GPT metadata and every fixed
   partition that had to move. Reboot back to TWRP, format the donor, then flash
   a matching system/vendor image. Do not boot Android if the script stopped
   after a GPT write; use the saved files and the scatter/MTK recovery path.

The default ADB is `..\platform-tools\adb.exe`; pass `-Adb` and `-Serial` when
needed. The script is deliberately not run against normal Android and does
not reboot or flash boot/vendor/lk/dtbo.

## Validation status

The planner is designed to be exercised first against captured GPT images and
then on the dedicated test device in TWRP. A real device run is not started
while it is in normal Android: its shell cannot read the block geometry and
would fail the recovery/root checks before any write.

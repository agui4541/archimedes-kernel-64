# Archimedes connectivity modules

This kernel tree keeps the common kernel baseline independent of KernelSU. The
pure branch and the "kernelsu" branch must use the same connectivity module
inputs; KernelSU is an overlay, not a second driver fork.

## Current failure

The retail vendor image contains the four expected modules under
/vendor/lib/modules, but they are 32-bit ARM modules (ARMv7 p2v8) while
the adapted kernel is AArch64. The adapted kernel therefore starts with no
loaded WMT/WLAN/BT modules, /dev/wmtdetect is absent, and
vendor.connsys.driver.ready remains unset. This is why the framework sees no
Wi-Fi interface and the Bluetooth HAL aborts before controller initialization.

## Pinned public inputs

The sources are intentionally not vendored into this repository. They are
fetched at fixed commits during a controlled build, preserving the public
kernel repository without redistributing a large third-party source snapshot.

- WMT/common: MotorolaMobilityLLC/vendor-mediatek-kernel_modules-connectivity-common
  branch android-9-release-poa, commit 364afcf.
- Wi-Fi character adapter: MotorolaMobilityLLC/vendor-mediatek-kernel_modules-connectivity-wlan-adaptor
  branch android-9-release-poa, commit e84d47e.
- Bluetooth character driver: MotorolaMobilityLLC/vendor-mediatek-kernel_modules-connectivity-bt-mt66xx
  branch android-9-release-poa, commit 9074126.
- Gen4M WLAN core: zainarbani/vendor-mediatek-kernel_modules-connectivity-wlan-core-gen4m
  commit ba2c5a5.

The upstream files carry their own GPL/BSD notices. Keep those notices with any
local source checkout and re-check them before publishing a vendored snapshot.

## 4.9 compatibility notes

The MT6761 4.9 build currently needs a small, separately reviewable patch set:

- disable the WPA3 external-auth path because this kernel's cfg80211 has no
  external_auth API;
- omit the unavailable WMT MPU lock hooks;
- add the WLAN-driver-own callback member expected by the Gen4M source;
- provide the optional get_low_latency_mode() fallback returning zero.

These are compatibility shims, not KernelSU changes. They must be applied to the
same external source checkout for both branches.

## Build outputs

A successful AArch64 build produces:

- wmt_drv.ko
- wmt_chrdev_wifi.ko
- wlan_6761_axi.ko (installed as wlan_drv_gen4m.ko)
- bt_drv.ko

All four modules must report the same 4.9.117 AArch64 vermagic and be loaded in
dependency order: wmt_drv, bt_drv, wmt_chrdev_wifi, then wlan_drv_gen4m. Do not
copy the old ARMv7 vendor modules into an AArch64 kernel image.

## Branch policy

Driver/config/documentation commits land on main first. kernelsu merges main
and only then adds KernelSU-specific commits. This keeps the two driver baselines
synchronized and makes a pure-kernel regression bisectable.

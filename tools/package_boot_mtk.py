#!/usr/bin/env python3
"""Package a KernelSU kernel with the known-good MTK v1 boot header/DTB.

This is intentionally strict: the device's LK checks the DT overlay symbols,
and a seemingly valid Android boot image can still fail before the kernel runs.
"""

from __future__ import annotations

import argparse
import hashlib
import struct
from pathlib import Path


PAGE_SIZE = 2048
BOOT_IMAGE_SIZE = 32 * 1024 * 1024
FDT_MAGIC = 0xD00DFEED
KERNEL_ADDR = 0x40080000
TAGS_ADDR = 0x47880000
REQUIRED_BOOTOPT = "bootopt=64S3,32S1,64S1"


def u32(buf: bytes, off: int) -> int:
    return struct.unpack_from("<I", buf, off)[0]


def cstring(buf: bytes, off: int, size: int) -> str:
    return buf[off : off + size].split(b"\0", 1)[0].decode("ascii", "replace")


def kernel_blob(image: bytes) -> bytes:
    if image[:8] != b"ANDROID!":
        raise ValueError("not an Android boot image")
    if u32(image, 36) != PAGE_SIZE:
        raise ValueError(f"unexpected page size: {u32(image, 36)}")
    size = u32(image, 8)
    end = PAGE_SIZE + size
    if end > len(image):
        raise ValueError("KERNEL_SZ exceeds image")
    return image[PAGE_SIZE:end]


def dtb_suffix(blob: bytes) -> tuple[bytes, int]:
    """Return the complete FDT suffix (some MTK images contain two FDTs)."""
    magic = struct.pack(">I", FDT_MAGIC)
    pos = blob.find(magic)
    while pos >= 0:
        cur = pos
        count = 0
        while cur + 8 <= len(blob) and blob[cur : cur + 4] == magic:
            total = struct.unpack_from(">I", blob, cur + 4)[0]
            if total < 40 or cur + total > len(blob):
                break
            cur += total
            count += 1
        if count and cur == len(blob):
            return blob[pos:], pos
        pos = blob.find(magic, pos + 1)
    raise ValueError("no complete FDT suffix reaches the end of KERNEL_SZ")


def validate_baseline(image: bytes, label: str, required_symbol: bytes) -> bytes:
    if image[:8] != b"ANDROID!":
        raise ValueError(f"{label}: not an Android boot image")
    if u32(image, 12) != KERNEL_ADDR:
        raise ValueError(f"{label}: kernel_addr must be 0x{KERNEL_ADDR:08x}")
    if u32(image, 32) != TAGS_ADDR:
        raise ValueError(f"{label}: tags_addr must be 0x{TAGS_ADDR:08x}")
    if u32(image, 1644) != 1648:
        raise ValueError(f"{label}: expected Android boot header v1 (header_size=1648)")
    cmdline = cstring(image, 64, 512) + " " + cstring(image, 608, 1024)
    if REQUIRED_BOOTOPT not in cmdline:
        raise ValueError(f"{label}: missing {REQUIRED_BOOTOPT!r}")
    dtb, _ = dtb_suffix(kernel_blob(image))
    if required_symbol and required_symbol not in dtb:
        raise ValueError(f"{label}: DTB lacks required symbol {required_symbol!r}")
    return dtb


def validate_dtb(dtb: bytes, label: str, required_symbol: bytes) -> bytes:
    """Validate a supplied complete DTB suffix without rewriting it."""
    suffix, offset = dtb_suffix(dtb)
    if offset != 0:
        raise ValueError(f"{label}: leading bytes before DTB suffix")
    if required_symbol and required_symbol not in suffix:
        raise ValueError(f"{label}: DTB lacks required symbol {required_symbol!r}")
    return suffix


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--good-boot", type=Path, required=True,
                    help="last known-good boot image; supplies header and DTB")
    source = ap.add_mutually_exclusive_group(required=True)
    source.add_argument("--ksu-boot", type=Path,
                        help="boot image containing the newly built KSU kernel")
    source.add_argument("--ksu-kernel", type=Path,
                        help="pure gzip Image.gz produced by the kernel build")
    ap.add_argument("--dtb", type=Path,
                    help="complete DTB suffix; otherwise preserve it from --good-boot")
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--required-dtb-symbol", default="aw87329_pa")
    args = ap.parse_args()

    good = args.good_boot.read_bytes()
    required_symbol = args.required_dtb_symbol.encode()
    display_dtb = validate_baseline(good, "good boot", required_symbol)
    if args.dtb:
        display_dtb = validate_dtb(args.dtb.read_bytes(), "supplied DTB", required_symbol)
    if args.ksu_kernel:
        ksu_kernel = args.ksu_kernel.read_bytes()
        try:
            _embedded_dtb, _ = dtb_suffix(ksu_kernel)
        except ValueError:
            pass
        else:
            raise ValueError("--ksu-kernel must be pure Image.gz, not Image.gz-dtb")
    else:
        ksu = args.ksu_boot.read_bytes()
        _ksu_dtb, ksu_dtb_offset = dtb_suffix(kernel_blob(ksu))
        ksu_kernel = kernel_blob(ksu)[:ksu_dtb_offset]
    if not ksu_kernel.startswith(b"\x1f\x8b"):
        raise ValueError("KSU kernel payload is not gzip")

    combined = ksu_kernel + display_dtb
    if PAGE_SIZE + len(combined) > BOOT_IMAGE_SIZE:
        raise ValueError("combined kernel does not fit in 32 MiB")
    out = bytearray(BOOT_IMAGE_SIZE)
    out[:PAGE_SIZE] = good[:PAGE_SIZE]
    struct.pack_into("<I", out, 8, len(combined))
    out[PAGE_SIZE : PAGE_SIZE + len(combined)] = combined
    # Android boot image ID is SHA-1(kernel || ramdisk || second || recovery_dtbo).
    out[576:608] = hashlib.sha1(combined).digest() + bytes(12)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(out)

    # Post-write invariants are deliberately part of the command's success path.
    if args.output.stat().st_size != BOOT_IMAGE_SIZE:
        raise ValueError("output is not exactly 32 MiB")
    if dtb_suffix(bytes(out[PAGE_SIZE : PAGE_SIZE + len(combined)]))[0] != display_dtb:
        raise ValueError("output DTB changed during packaging")
    print(f"output={args.output}")
    print(f"kernel_size={len(combined)} pure_ksu={len(ksu_kernel)} dtb={len(display_dtb)}")
    print(f"sha256={hashlib.sha256(out).hexdigest()}")


if __name__ == "__main__":
    main()

#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
VARIANT=${1:-pure}
JOBS=${JOBS:-24}
OUT=${OUT:-"$ROOT/out-archimedes-$VARIANT"}
CROSS_COMPILE=${CROSS_COMPILE:-aarch64-linux-gnu-}
HOSTCFLAGS=${HOSTCFLAGS:--fcommon}
HOSTCXXFLAGS=${HOSTCXXFLAGS:--fcommon}
LOCALVERSION=${LOCALVERSION:--agui@caner}
KBUILD_BUILD_USER=${KBUILD_BUILD_USER:-agui}
KBUILD_BUILD_HOST=${KBUILD_BUILD_HOST:-caner}
if [[ -z "${KBUILD_BUILD_TIMESTAMP:-}" ]]; then
  if [[ -n "${SOURCE_DATE_EPOCH:-}" ]]; then
    KBUILD_BUILD_TIMESTAMP=$(date -u -d "@${SOURCE_DATE_EPOCH}" '+%a %b %d %H:%M:%S UTC %Y')
  else
    KBUILD_BUILD_TIMESTAMP=$(date -u '+%a %b %d %H:%M:%S UTC %Y')
  fi
fi
export KBUILD_BUILD_TIMESTAMP

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
command -v make >/dev/null 2>&1 || die 'make is required'
command -v python3 >/dev/null 2>&1 || die 'python3 is required'
command -v "${CROSS_COMPILE}gcc" >/dev/null 2>&1 || die "missing ${CROSS_COMPILE}gcc"
[[ "$OUT" != "$ROOT" && "$OUT" != / && -n "$OUT" ]] || die 'unsafe OUT path'

# The checkout contains a materialized KernelSU copy used by Kbuild. Keep the
# generated rc hook in lockstep with the source copy; otherwise a source edit
# can silently miss the actual build input.
if ! cmp -s "$ROOT/KernelSU/kernel/ksud.c" "$ROOT/drivers/kernelsu/ksud.c"; then
  die 'KernelSU ksud.c copies differ; sync KernelSU/kernel and drivers/kernelsu before building'
fi

case "$VARIANT" in
  pure)
    DEFCONFIG=k61v1_64_archimedes_defconfig
    ;;
  ksu)
    DEFCONFIG=k61v1_64_archimedes_defconfig
    [[ -f "$ROOT/drivers/kernelsu/Kconfig" ]] || die 'this checkout has no materialized KernelSU tree'
    ;;
  *) die "usage: $0 pure|ksu" ;;
esac

if [[ "${CLEAN_BUILD:-1}" = 1 ]]; then
  rm -rf -- "$OUT"
fi
mkdir -p "$OUT/include"

# Old 4.9 Kbuild invokes the DT preprocessor with -I./include, where ./ is O=.
# Keep the source DT bindings visible without copying generated headers.
if [[ -e "$OUT/include/dt-bindings" && ! -L "$OUT/include/dt-bindings" ]]; then
  rm -rf -- "$OUT/include/dt-bindings"
fi
if [[ ! -e "$OUT/include/dt-bindings" ]]; then
  ln -s "$ROOT/include/dt-bindings" "$OUT/include/dt-bindings"
fi

MAKE=(make -C "$ROOT" O="$OUT" ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE"
  HOSTCFLAGS="$HOSTCFLAGS" HOSTCXXFLAGS="$HOSTCXXFLAGS")
"${MAKE[@]}" "$DEFCONFIG"

CONFIG_TOOL="$OUT/scripts/config"
[[ -x "$CONFIG_TOOL" ]] || CONFIG_TOOL="$ROOT/scripts/config"
[[ -x "$CONFIG_TOOL" ]] || die 'scripts/config was not built'
if [[ "$VARIANT" = ksu ]]; then
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_KSU
  # KernelSU's standard mount flow requires mount namespaces.  The original
  # Archimedes defconfig disables the namespace subsystem, which makes
  # setns(/proc/1/ns/mnt) and propagation operations fail on the GSI.
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_NAMESPACES
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_UTS_NS
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_PID_NS
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_NET_NS
  "$CONFIG_TOOL" --file "$OUT/.config" --enable CONFIG_IPC_NS
else
  # Keep the pure branch genuinely free of KernelSU even if the shared defconfig
  # or a stale configuration enables it by default.
  "$CONFIG_TOOL" --file "$OUT/.config" --disable CONFIG_KSU
fi
"${MAKE[@]}" olddefconfig

"${MAKE[@]}" LOCALVERSION="$LOCALVERSION" KBUILD_BUILD_USER="$KBUILD_BUILD_USER" \
  KBUILD_BUILD_HOST="$KBUILD_BUILD_HOST" KBUILD_BUILD_TIMESTAMP="$KBUILD_BUILD_TIMESTAMP" \
  -j"$JOBS" Image.gz-dtb

{
  printf 'variant=%s\n' "$VARIANT"
  printf 'jobs=%s\n' "$JOBS"
  printf 'localversion=%s\n' "$LOCALVERSION"
  printf 'build_user=%s\n' "$KBUILD_BUILD_USER"
  printf 'build_host=%s\n' "$KBUILD_BUILD_HOST"
  printf 'build_timestamp=%s\n' "$KBUILD_BUILD_TIMESTAMP"
  printf 'cross_compile=%s\n' "$CROSS_COMPILE"
  git -C "$ROOT" rev-parse HEAD 2>/dev/null | sed 's/^/source_commit=/' || printf 'source_commit=archive\n'
  "${MAKE[@]}" LOCALVERSION="$LOCALVERSION" -s kernelrelease | sed 's/^/kernelrelease=/'
  sha256sum "$OUT/arch/arm64/boot/Image.gz-dtb" | sed 's# .*#  Image.gz-dtb#'
} > "$OUT/BUILD-INFO.txt"

printf 'built=%s\n' "$OUT/arch/arm64/boot/Image.gz-dtb"

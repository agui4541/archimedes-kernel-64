//! Minimal KernelSU prctl ABI wrappers for legacy Android kernels.
//!
//! The upstream KernelSU rustix fork is not available from crates.io. These
//! wrappers intentionally mirror the ABI in `kernel/ksu.h` and keep ksud
//! buildable for armv7 Android.

use std::ffi::c_void;

const KERNEL_SU_OPTION: libc::c_int = 0xDEAD_BEEF_u32 as libc::c_int;
const CMD_GET_VERSION: libc::c_ulong = 2;
const CMD_REPORT_EVENT: libc::c_ulong = 7;
const CMD_SET_SEPOLICY: libc::c_ulong = 8;
const CMD_CHECK_SAFEMODE: libc::c_ulong = 9;

#[inline]
unsafe fn call(cmd: libc::c_ulong, arg3: libc::c_ulong, arg4: libc::c_ulong, arg5: libc::c_ulong) -> libc::c_int {
    libc::prctl(KERNEL_SU_OPTION, cmd, arg3, arg4, arg5)
}

pub fn get_version() -> i32 {
    let mut version = 0_u32;
    let mut lkm = 0_u32;
    unsafe {
        call(
            CMD_GET_VERSION,
            (&mut version as *mut u32).cast::<c_void>() as libc::c_ulong,
            (&mut lkm as *mut u32).cast::<c_void>() as libc::c_ulong,
            0,
        );
    }
    version as i32
}

pub fn report_event(event: u64) {
    unsafe { call(CMD_REPORT_EVENT, event as libc::c_ulong, 0, 0); }
}

pub fn check_kernel_safemode() -> bool {
    let mut result = 0_u32;
    unsafe {
        call(
            CMD_CHECK_SAFEMODE,
            0,
            0,
            (&mut result as *mut u32).cast::<c_void>() as libc::c_ulong,
        );
    }
    result == KERNEL_SU_OPTION as u32
}

pub fn set_policy(policy: *const c_void) -> bool {
    let mut result = 0_u32;
    unsafe {
        call(
            CMD_SET_SEPOLICY,
            policy as libc::c_ulong,
            0,
            (&mut result as *mut u32).cast::<c_void>() as libc::c_ulong,
        );
    }
    result == KERNEL_SU_OPTION as u32
}

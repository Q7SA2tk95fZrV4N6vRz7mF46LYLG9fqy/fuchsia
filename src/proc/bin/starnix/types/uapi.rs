// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

use zerocopy::{AsBytes, FromBytes};

use crate::types::UserAddress;

#[cfg(target_arch = "x86_64")]
use linux_uapi::x86_64 as uapi;
pub use uapi::*;

pub type dev_t = uapi::__kernel_old_dev_t;
pub type gid_t = uapi::__kernel_gid_t;
pub type ino_t = uapi::__kernel_ino_t;
pub type mode_t = uapi::__kernel_mode_t;
pub type off_t = uapi::__kernel_off_t;
pub type pid_t = uapi::__kernel_pid_t;
pub type uid_t = uapi::__kernel_uid_t;

#[derive(Debug, Eq, PartialEq, Hash, Copy, Clone, AsBytes, FromBytes)]
#[repr(C)]
pub struct utsname_t {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
}

#[derive(Debug, Default, Clone, Copy, AsBytes, FromBytes)]
#[repr(C)]
pub struct stat_t {
    pub st_dev: dev_t,
    pub st_ino: ino_t,
    pub st_nlink: u64,
    pub st_mode: mode_t,
    pub st_uid: uid_t,
    pub st_gid: gid_t,
    pub _pad0: u32,
    pub st_rdev: dev_t,
    pub st_size: off_t,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atim: timespec,
    pub st_mtim: timespec,
    pub st_ctim: timespec,
    pub _pad3: [i64; 3],
}

#[derive(Debug, Default, Clone, Copy, AsBytes, FromBytes, PartialEq)]
#[repr(C)]
pub struct statfs {
    pub f_type: i64,
    pub f_bsize: u64,
    pub f_blocks: i64,
    pub f_bfree: i64,
    pub f_bavail: i64,
    pub f_files: i64,
    pub f_ffree: i64,
    pub f_fsid: i64,
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, AsBytes, FromBytes)]
#[repr(C)]
pub struct sigaltstack_t {
    pub ss_sp: UserAddress,
    pub ss_flags: u32,
    pub _pad0: u32,
    pub ss_size: usize,
}

impl sigaltstack_t {
    /// Returns true if the passed in `ptr` is considered to be on this stack.
    pub fn contains_pointer(&self, ptr: u64) -> bool {
        let min = self.ss_sp.ptr() as u64;
        let max = (self.ss_sp + self.ss_size).ptr() as u64;
        ptr >= min && ptr <= max
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, AsBytes, FromBytes)]
#[repr(C)]
pub struct sigaction_t {
    pub sa_handler: UserAddress,
    pub sa_flags: uapi::c_ulong,
    pub sa_restorer: UserAddress,
    pub sa_mask: sigset_t,
}

pub use uapi::__SIGRTMIN as SIGRTMIN;

pub const SIG_DFL: UserAddress = UserAddress::from(0);
pub const SIG_IGN: UserAddress = UserAddress::from(1);
pub const SIG_ERR: UserAddress = UserAddress::from(u64::MAX);

// Note that all the socket-related numbers below are defined in sys/socket.h, which is why they
// can't be generated by bindgen.

pub const AF_UNSPEC: uapi::__kernel_sa_family_t = 0;
pub const AF_UNIX: uapi::__kernel_sa_family_t = 1;
pub const AF_INET: uapi::__kernel_sa_family_t = 2;

pub const SOCK_CLOEXEC: u32 = O_CLOEXEC;
pub const SOCK_NONBLOCK: u32 = O_NONBLOCK;

pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;
pub const SOCK_SEQPACKET: u32 = 5;

pub const SHUT_RD: u32 = 0;
pub const SHUT_WR: u32 = 1;
pub const SHUT_RDWR: u32 = 2;

pub const SCM_RIGHTS: u32 = 1;
pub const SCM_CREDENTIALS: u32 = 2;
pub const SCM_SECURITY: u32 = 3;
/// The maximum number of bytes that the file descriptor array can occupy.
pub const SCM_MAX_FD: usize = 253;

pub const MSG_PEEK: u32 = 2;
pub const MSG_DONTROUTE: u32 = 4;
pub const MSG_TRYHARD: u32 = 4;
pub const MSG_CTRUNC: u32 = 8;
pub const MSG_PROBE: u32 = 0x10;
pub const MSG_TRUNC: u32 = 0x20;
pub const MSG_DONTWAIT: u32 = 0x40;
pub const MSG_EOR: u32 = 0x80;
pub const MSG_WAITALL: u32 = 0x100;
pub const MSG_FIN: u32 = 0x200;
pub const MSG_SYN: u32 = 0x400;
pub const MSG_CONFIRM: u32 = 0x800;
pub const MSG_RST: u32 = 0x1000;
pub const MSG_ERRQUEUE: u32 = 0x2000;
pub const MSG_NOSIGNAL: u32 = 0x4000;
pub const MSG_MORE: u32 = 0x8000;
pub const MSG_WAITFORONE: u32 = 0x10000;
pub const MSG_BATCH: u32 = 0x40000;
pub const MSG_FASTOPEN: u32 = 0x20000000;
pub const MSG_CMSG_CLOEXEC: u32 = 0x40000000;
pub const MSG_EOF: u32 = MSG_FIN;
pub const MSG_CMSG_COMPAT: u32 = 0;

#[repr(C)]
#[derive(Debug, Copy, Clone, AsBytes, FromBytes)]
pub struct sockaddr_un {
    pub sun_family: uapi::__kernel_sa_family_t,
    pub sun_path: [u8; 108],
}

impl Default for sockaddr_un {
    fn default() -> Self {
        sockaddr_un { sun_family: 0, sun_path: [0; 108] }
    }
}

pub type socklen_t = u32;

#[derive(Clone, Debug, Default, PartialEq, AsBytes, FromBytes)]
#[repr(C)]
pub struct ucred {
    pub pid: pid_t,
    pub uid: uid_t,
    pub gid: gid_t,
}

#[derive(Debug, Default, Clone, AsBytes, FromBytes)]
#[repr(C)]
pub struct msghdr {
    pub msg_name: UserAddress,
    pub msg_namelen: u64,
    pub msg_iov: UserAddress,
    pub msg_iovlen: u64,
    pub msg_control: UserAddress,
    pub msg_controllen: usize,
    pub msg_flags: u64,
}

#[derive(AsBytes, FromBytes, Default)]
#[repr(packed)]
pub struct cmsghdr {
    pub cmsg_len: usize,
    pub cmsg_level: u32,
    pub cmsg_type: u32,
}

#[derive(Debug, Default, Clone, AsBytes, FromBytes)]
#[repr(C)]
pub struct mmsghdr {
    pub msg_hdr: msghdr,
    pub msg_len: u32,
    pub __reserved: [u8; 4usize],
}

#[derive(Debug, Default, Copy, Clone, AsBytes, FromBytes)]
#[repr(C)]
pub struct linger {
    pub l_onoff: i32,
    pub l_linger: i32,
}

pub const EFD_CLOEXEC: u32 = O_CLOEXEC;
pub const EFD_NONBLOCK: u32 = O_NONBLOCK;
pub const EFD_SEMAPHORE: u32 = 1;

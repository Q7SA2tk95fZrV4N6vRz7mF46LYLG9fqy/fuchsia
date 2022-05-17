// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(fxb/97355): Remove once the device is complete.
#![allow(dead_code)]

use {
    bitflags::bitflags,
    zerocopy::{LittleEndian, U16, U32, U64},
};

pub type LE16 = U16<LittleEndian>;
pub type LE32 = U32<LittleEndian>;
pub type LE64 = U64<LittleEndian>;

// 5.10.2 Virtqueues
//
// Note that these queues are from the perspective of the driver, so the RX queue is for device
// to driver communication, etc.
pub const RX_QUEUE_IDX: u16 = 0;
pub const TX_QUEUE_IDX: u16 = 1;
pub const EVENT_QUEUE_IDX: u16 = 2;

// 5.10.6.5 Stream Sockets
//
// These flags are hints about future connection behavior. Note that these flags are permanent once
// received, and subsequent packets without these flags do not clear these flags.
bitflags! {
    #[derive(Default)]
    pub struct VirtioVsockFlags: u32 {
        const SHUTDOWN_RECIEVE = 0b01;
        const SHUTDOWN_SEND    = 0b10;
        const SHUTDOWN_BOTH    = Self::SHUTDOWN_RECIEVE.bits | Self::SHUTDOWN_SEND.bits;
    }
}

// 5.10.4 Device configuration layout
#[repr(C, packed)]
pub struct VirtioVsockConfig {
    pub guest_cid: LE64,
}

impl VirtioVsockConfig {
    pub fn new_with_default_cid() -> Self {
        Self { guest_cid: LE64::new(fidl_fuchsia_virtualization::DEFAULT_GUEST_CID.into()) }
    }
}

// 5.10.6 Device Operation
#[repr(C, packed)]
#[derive(Default)]
pub struct VirtioVsockHeader {
    pub src_cid: LE64,
    pub dst_cid: LE64,
    pub src_port: LE32,
    pub dst_port: LE32,
    pub len: LE32,
    pub vsock_type: LE16,
    pub op: LE16,
    pub flags: LE32,
    pub buf_alloc: LE32,
    pub fwd_cnt: LE32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OpType {
    Invalid = 0,       // VIRTIO_VSOCK_OP_INVALID
    Request = 1,       // VIRTIO_VSOCK_OP_REQUEST
    Response = 2,      // VIRTIO_VSOCK_OP_RESPONSE
    Reset = 3,         // VIRTIO_VSOCK_OP_RST
    Shutdown = 4,      // VIRTIO_VSOCK_OP_SHUTDOWN
    ReadWrite = 5,     // VIRTIO_VSOCK_OP_RW
    CreditUpdate = 6,  // VIRTIO_VSOCK_OP_CREDIT_UPDATE
    CreditRequest = 7, // VIRTIO_VSOCK_OP_CREDIT_REQUEST
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia netdevice client library

#![deny(missing_docs)]
pub mod client;
pub mod error;
pub mod port_slab;
pub mod session;

pub use client::{Client, DevicePortEvent, PortStatus};
pub use error::{Error, Result};
pub use port_slab::PortSlab;
pub use session::{Buffer, Config, DeviceInfo, Port, Session, Task};

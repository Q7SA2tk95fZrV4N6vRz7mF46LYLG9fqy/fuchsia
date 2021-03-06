// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    super::subcommands::{
        debug_bind::args::DebugBindCommand, device::args::DeviceCommand, dump::args::DumpCommand,
        i2c::args::I2cCommand, list::args::ListCommand, list_devices::args::ListDevicesCommand,
        list_hosts::args::ListHostsCommand, lsblk::args::LsblkCommand, lspci::args::LspciCommand,
        lsusb::args::LsusbCommand, print_input_report::args::PrintInputReportCommand,
        register::args::RegisterCommand, restart::args::RestartCommand,
        runtool::args::RunToolCommand,
    },
    argh::FromArgs,
};

#[derive(FromArgs, Debug, PartialEq)]
#[argh(name = "driver", description = "Support driver development workflows")]
pub struct DriverCommand {
    #[argh(subcommand)]
    pub subcommand: DriverSubCommand,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum DriverSubCommand {
    DebugBind(DebugBindCommand),
    Device(DeviceCommand),
    Dump(DumpCommand),
    I2c(I2cCommand),
    List(ListCommand),
    ListDevices(ListDevicesCommand),
    ListHosts(ListHostsCommand),
    Lsblk(LsblkCommand),
    Lspci(LspciCommand),
    Lsusb(LsusbCommand),
    PrintInputReport(PrintInputReportCommand),
    Register(RegisterCommand),
    Restart(RestartCommand),
    RunTool(RunToolCommand),
}

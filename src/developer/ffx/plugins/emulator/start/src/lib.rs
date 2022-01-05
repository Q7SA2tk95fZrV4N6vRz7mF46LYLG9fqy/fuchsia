// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::pbm::make_configs;
use anyhow::{Context, Result};
use ffx_core::ffx_plugin;
use ffx_emulator_common::config::FfxConfigWrapper;
use ffx_emulator_engines::EngineBuilder;
use ffx_emulator_start_args::StartCommand;
use fidl_fuchsia_developer_bridge as bridge;

mod pbm;
mod template_helpers;

#[ffx_plugin("emu.experimental")]
pub async fn start(cmd: StartCommand, _daemon_proxy: bridge::DaemonProxy) -> Result<()> {
    let config = FfxConfigWrapper::new();
    let emulator_configuration =
        make_configs(&cmd, &config).await.context("making configuration from metadata")?;

    // Initialize an engine of the requested type with the configuration defined in the manifest.
    let result =
        EngineBuilder::new().config(emulator_configuration).engine_type(cmd.engine).build().await;

    std::process::exit(match result {
        Ok(mut engine) => engine.start().await?,
        Err(e) => {
            println!("{:?}", e);
            1
        }
    });
}

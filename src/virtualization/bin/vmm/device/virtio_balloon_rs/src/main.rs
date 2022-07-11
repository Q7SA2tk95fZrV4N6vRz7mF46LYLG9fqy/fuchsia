// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod wire;

use {
    anyhow::{anyhow, Context},
    fidl::endpoints::RequestStream,
    fidl_fuchsia_virtualization_hardware::VirtioBalloonRequestStream,
    fuchsia_component::server,
    futures::{StreamExt, TryFutureExt, TryStreamExt},
    tracing,
};

async fn run_virtio_balloon(
    mut virtio_balloon_fidl: VirtioBalloonRequestStream,
) -> Result<(), anyhow::Error> {
    // Receive start info as first message.
    let (start_info, responder) = virtio_balloon_fidl
        .try_next()
        .await?
        .ok_or(anyhow!("Failed to read fidl message from the channel."))?
        .into_start()
        .ok_or(anyhow!("Start should be the first message sent."))?;

    // Prepare the device builder
    let (device_builder, guest_mem) = machina_virtio_device::from_start_info(start_info)?;

    responder.send()?;

    // Complete the setup of queues and get a device.
    let mut virtio_device_fidl = virtio_balloon_fidl.cast_stream();
    let (device, ready_responder) = machina_virtio_device::config_builder_from_stream(
        device_builder,
        &mut virtio_device_fidl,
        &[wire::INFLATEQ, wire::DEFLATEQ, wire::STATSQ][..],
        &guest_mem,
    )
    .await
    .context("Failed to initialize device.")?;

    // Initialize all queues.
    let _inflate_stream = device.take_stream(wire::INFLATEQ)?;
    let _deflate_stream = device.take_stream(wire::DEFLATEQ)?;
    let _stats_stream = device.take_stream(wire::STATSQ)?;
    ready_responder.send()?;

    futures::try_join!(device
        .run_device_notify(virtio_device_fidl)
        .map_err(|e| anyhow!("run_device_notify: {}", e)),)?;
    Ok(())
}

#[fuchsia::main(logging = true, threads = 1)]
async fn main() -> Result<(), anyhow::Error> {
    let mut fs = server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: VirtioBalloonRequestStream| stream);
    fs.take_and_serve_directory_handle().context("Error starting server")?;
    fs.for_each_concurrent(None, |stream| async {
        if let Err(e) = run_virtio_balloon(stream).await {
            tracing::error!("Error running virtio_balloon service: {}", e);
        }
    })
    .await;
    Ok(())
}

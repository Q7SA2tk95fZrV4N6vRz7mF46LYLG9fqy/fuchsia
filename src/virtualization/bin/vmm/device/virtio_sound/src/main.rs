// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod audio_streams;
mod notification;
mod reply;
mod sequencer;
mod service;
mod wire;
mod wire_convert;

use {
    crate::wire::{LE32, LE64},
    crate::wire_convert::*,
    anyhow::{anyhow, Context, Error},
    fidl::endpoints::RequestStream,
    fidl_fuchsia_virtualization_hardware::{VirtioSoundRequest, VirtioSoundRequestStream},
    futures::{StreamExt, TryFutureExt, TryStreamExt},
    once_cell::sync::Lazy,
    service::VirtSoundService,
    std::rc::Rc,
    std::vec::Vec,
    tracing,
    virtio_device::chain::ReadableChain,
};

// There are no defined features. See: 5.14.3 Feature Bits
// https://www.kraxel.org/virtio/virtio-v1.1-cs01-sound-v8.html#x1-4980003
const FEATURES: u32 = 0;

// This comes from Intel HDA 7.3.3.31
// https://www.intel.com/content/dam/www/public/us/en/documents/product-specifications/high-definition-audio-specification.pdf
//
// bits 31:30 Port Connectivity   = 2     ("fixed-function device")
// bits 29:24 Location            = (1<4) ("internal" x "N/A"
// bits 23:20 Default Device      = 1     ("Speaker")
// bits 19:16 Connection Type     = 0     ("Unknown")
// bits 15:12 Color               = 0     ("Unknown")
// bits 11:08 Misc                = 0     (unset)
// bits 07:04 Default Association = 1     (arbitrary, but 0 is reserved)
// bits 03:00 Sequence            = 0     (arbitrary)
//
// Which is 2<<30 | (1<<4)<<24 | 1<<20 | 1<<4
const JACK_HDA_REG_DEFCONF: u32 = 0x90100010;

// This comes from Intel HDA 7.3.4.9
// https://www.intel.com/content/dam/www/public/us/en/documents/product-specifications/high-definition-audio-specification.pdf
// Only bits 4 and 5 are set.
const JACK_HDA_REG_CAPS: u32 = (1 << 5) | (1 << 4);

// The following consts declare our static configuration of jacks, streams, and channel maps.
// We support a single "built-in" jack, which is never unplugged, that contains a single output
// stream and a single input stream. This simulates a device with one built-in speaker and one
// built-in microphone. Streams can be mono or stero.
static JACKS: Lazy<Vec<wire::VirtioSndJackInfo>> = Lazy::new(|| {
    vec![wire::VirtioSndJackInfo {
        hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
        features: LE32::new(0),
        hda_reg_defconf: LE32::new(JACK_HDA_REG_DEFCONF),
        hda_reg_caps: LE32::new(JACK_HDA_REG_CAPS),
        connected: 1,
        padding: Default::default(),
    }]
});

static STREAMS: Lazy<Vec<wire::VirtioSndPcmInfo>> = Lazy::new(|| {
    vec![
        wire::VirtioSndPcmInfo {
            hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
            // TODO(fxbug.dev/87645): support?
            // VIRTIO_SND_PCM_F_MSG_POLLING
            // VIRTIO_SND_PCM_F_EVT_SHMEM_PERIODS
            // VIRTIO_SND_PCM_F_EVT_XRUNS
            features: LE32::new(0),
            formats: LE64::new(*WIRE_FORMATS_SUPPORTED_BITMASK),
            rates: LE64::new(*WIRE_RATES_SUPPORTED_BITMASK),
            direction: wire::VIRTIO_SND_D_OUTPUT,
            channels_min: 1, // mono or stereo
            channels_max: 2,
            padding: Default::default(),
        },
        wire::VirtioSndPcmInfo {
            hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
            // TODO(fxbug.dev/87645): support?
            // VIRTIO_SND_PCM_F_MSG_POLLING
            // VIRTIO_SND_PCM_F_EVT_SHMEM_PERIODS
            // VIRTIO_SND_PCM_F_EVT_XRUNS
            features: LE32::new(0),
            formats: LE64::new(*WIRE_FORMATS_SUPPORTED_BITMASK),
            rates: LE64::new(*WIRE_RATES_SUPPORTED_BITMASK),
            direction: wire::VIRTIO_SND_D_INPUT,
            channels_min: 1, // mono only; stereo input is uncommon
            channels_max: 1,
            padding: Default::default(),
        },
    ]
});

static MONO: Lazy<[u8; wire::VIRTIO_SND_CHMAP_MAX_SIZE]> = Lazy::new(|| {
    let mut out: [u8; wire::VIRTIO_SND_CHMAP_MAX_SIZE] = Default::default();
    out[0] = wire::VIRTIO_SND_CHMAP_MONO;
    out
});

static STEREO: Lazy<[u8; wire::VIRTIO_SND_CHMAP_MAX_SIZE]> = Lazy::new(|| {
    let mut out: [u8; wire::VIRTIO_SND_CHMAP_MAX_SIZE] = Default::default();
    out[0] = wire::VIRTIO_SND_CHMAP_FL;
    out[1] = wire::VIRTIO_SND_CHMAP_FR;
    out
});

static CHMAPS: Lazy<Vec<wire::VirtioSndChmapInfo>> = Lazy::new(|| {
    vec![
        wire::VirtioSndChmapInfo {
            hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
            direction: wire::VIRTIO_SND_D_OUTPUT,
            channels: 1,
            positions: *MONO,
        },
        wire::VirtioSndChmapInfo {
            hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
            direction: wire::VIRTIO_SND_D_OUTPUT,
            channels: 2,
            positions: *STEREO,
        },
        wire::VirtioSndChmapInfo {
            hdr: wire::VirtioSndInfo { hda_fn_nid: LE32::new(0) },
            direction: wire::VIRTIO_SND_D_INPUT,
            channels: 1,
            positions: *MONO,
        },
    ]
});

static CONFIG: Lazy<wire::VirtioSndConfig> = Lazy::new(|| wire::VirtioSndConfig {
    jacks: LE32::new(JACKS.len() as u32),
    streams: LE32::new(STREAMS.len() as u32),
    chmaps: LE32::new(CHMAPS.len() as u32),
});

async fn run_virtio_sound(mut con: VirtioSoundRequestStream) -> Result<(), Error> {
    // First method call must be Start().
    let (start_info, audio, responder) = match con.try_next().await? {
        Some(VirtioSoundRequest::Start { start_info, audio, responder }) => {
            (start_info, audio, responder)
        }
        Some(msg) => {
            return Err(anyhow!("Expected Start message, got {:?}", msg));
        }
        None => {
            return Err(anyhow!("Expected Start message, got end of stream"));
        }
    };

    let (device_builder, guest_mem) = machina_virtio_device::from_start_info(start_info)?;
    responder.send(FEATURES, CONFIG.jacks.get(), CONFIG.streams.get(), CONFIG.chmaps.get())?;

    // Create the virtio device.
    let mut con = con.cast_stream();
    let (device, ready_responder) = machina_virtio_device::config_builder_from_stream(
        device_builder,
        &mut con,
        &[wire::CONTROLQ, wire::EVENTQ, wire::TXQ, wire::RXQ][..],
        &guest_mem,
    )
    .await
    .context("config_builder_from_stream")?;

    // Make sure each queue has been initialized.
    let controlq_stream = device.take_stream(wire::CONTROLQ)?;
    let txq_stream = device.take_stream(wire::TXQ)?;
    ready_responder.send()?;

    // Create a VirtSoundService to handle all virtq requests.
    let audio = audio.into_proxy()?;
    let vss = Rc::new(VirtSoundService::new(&JACKS, &STREAMS, &CHMAPS, &audio));

    tracing::info!(
        "Virtio sound device initialized with features = {:?}, config = {:?}",
        FEATURES,
        *CONFIG
    );

    // Process everything to completion.
    let mut txq_sequencer = sequencer::Sequencer::new();
    futures::try_join!(
        device.run_device_notify(con).err_into(),
        controlq_stream
            // Process controlq requests synchronously.
            .map(|chain| Ok((chain, vss.clone())))
            .try_for_each({
                let guest_mem = &guest_mem;
                move |(chain, vss)| async move {
                    vss.dispatch_controlq(ReadableChain::new(chain, guest_mem)).await
                }
            }),
        txq_stream
            // Attach a sequencer ticket to each message so we can order outgoing FIDL messages.
            // Process asynchronously so we can wait for FIDL replies concurrently.
            .map(|chain| Ok((chain, txq_sequencer.next(), vss.clone())))
            .try_for_each_concurrent(None /* unlimited concurrency */, {
                let guest_mem = &guest_mem;
                move |(chain, ticket, vss)| async move {
                    let lock = ticket.wait_turn().await;
                    vss.dispatch_txq(ReadableChain::new(chain, guest_mem), lock).await
                }
            }),
        vss.do_background_work(),
    )?;

    return Ok(());
}

#[fuchsia::component(logging = true, threads = 1)]
async fn main() -> Result<(), Error> {
    let mut fs = fuchsia_component::server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: VirtioSoundRequestStream| stream);
    fs.take_and_serve_directory_handle().context("Error starting server")?;

    fs.for_each_concurrent(None, |stream| async {
        if let Err(e) = run_virtio_sound(stream).await {
            tracing::error!("Error running virtio_sound service: {}", e);
        }
    })
    .await;
    Ok(())
}

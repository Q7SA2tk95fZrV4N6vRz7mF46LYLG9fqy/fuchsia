// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(fxbug.dev/87645): implement AudioInput types
#![allow(unused)]

use {
    crate::notification::Notification,
    crate::reply::*,
    crate::sequencer,
    crate::wire,
    crate::wire_convert,
    anyhow::{anyhow, Context, Error},
    async_trait::async_trait,
    fidl_fuchsia_media,
    fuchsia_async::{DurationExt, TimeoutExt},
    fuchsia_zircon::{self as zx, AsHandleRef},
    futures::FutureExt,
    futures::TryStreamExt,
    mapped_vmo::Mapping,
    scopeguard,
    std::cell::{Cell, RefCell},
    std::collections::{HashMap, VecDeque},
    std::ffi::CString,
    std::ops::Range,
    std::rc::Rc,
    tracing,
};

/// Parameters needed to construct an AudioStream.
#[derive(Debug, Copy, Clone)]
pub struct AudioStreamParams {
    pub stream_type: fidl_fuchsia_media::AudioStreamType,
    pub buffer_bytes: usize,
    pub period_bytes: usize,
}

/// Wraps a connection to the Fuchsia audio service. This abstract API covers
/// both output streams ("renderers") and input streams ("capturers").
#[async_trait(?Send)]
pub trait AudioStream<'a> {
    /// Creates a connection to the Fuchsia audio service.
    /// The stream must be disconnected.
    async fn connect(&self, params: AudioStreamParams) -> Result<(), Error>;

    /// Closes an existing connection.
    /// The stream must be connected.
    async fn disconnect(&self) -> Result<(), Error>;

    /// Starts streaming audio. Must have a valid connection.
    /// The stream must be connected.
    async fn start(&self) -> Result<(), Error>;

    /// Stops streaming audio. Must have a valid connection.
    /// The stream must be connected.
    async fn stop(&self) -> Result<(), Error>;

    /// Callback invoked each time a buffer of audio data is received from the guest driver.
    /// For output streams, the buffer contains audio to play.
    /// For input streams, the buffer is empty and should be filled.
    ///
    /// This method consumes `chain` and fully writes the response.
    ///
    /// This method will release `lock` just before the packet is transferred over FIDL. This
    /// ensures that FIDL packet delivery happens in the correct order while allowing us to
    /// wait concurrently on multiple pending packets.
    async fn on_receive_data<'b, 'c>(
        &self,
        chain: ReadableChain<'b, 'c>,
        lock: sequencer::Lock,
    ) -> Result<(), Error>;

    /// Performs background work. The returned future may never complete. To stop performing
    /// background work, drop the returned future.
    async fn do_background_work(&self) -> Result<(), Error>;
}

/// Construct an AudioStream that renders audio.
pub fn create_audio_output<'a>(
    audio: &'a fidl_fuchsia_media::AudioProxy,
) -> Box<dyn AudioStream<'a> + 'a> {
    // There is exactly one bg job per connect(), so a buffer size of 1 should be sufficient.
    let (send, recv) = tokio::sync::mpsc::channel(1);
    Box::new(AudioOutput {
        audio,
        conn: RefCell::new(None),
        bg_jobs_send: RefCell::new(send),
        bg_jobs_recv: RefCell::new(recv),
    })
}

/// Construct an AudioStream that captures audio.
pub fn create_audio_input<'a>(
    audio: &'a fidl_fuchsia_media::AudioProxy,
) -> Box<dyn AudioStream<'a> + 'a> {
    Box::new(AudioInput { audio, conn: RefCell::new(None) })
}

/// Represents a payload buffer used by an AudioRenderer or AudioCapturer.
/// Each PayloadBuffer is subdivided into fixed-sized packets.
///
/// Note on fragmentation: The virtio-sound spec has the following text:
///
///   5.14.6.8.1.2 Driver Requirements: Output Stream
///   The driver SHOULD populate the tx queue with period_bytes sized buffers.
///   The only exception is the end of the stream [which can be smaller].
///
///   5.14.6.8.2.2 Driver Requirements: Input Stream
///   The driver SHOULD populate the rx queue with period_bytes sized empty
///   buffers before starting the stream.
///
/// If both of these suggestions are followed, then every buffer received from
/// the driver will have size period_bytes (except for the very last packet
/// just before a STOP command). Packet allocation is simple: we pre-populate
/// a free list of fixed-sized packets.
///
/// Linux follows both suggestions. Hence, for simplicity, we assume that all
/// drivers behave like Linux's. In general, drivers don't need to behave like
/// Linux, so we can theoretically run into problems:
///
/// * The driver might send a buffer larger than period_bytes. If this happens,
///   we currently reject the buffer.
///
/// * The driver might send many buffers smaller than period_bytes. If this
///   happens, we may run out of packets and be forced to reject the buffer.
///
/// TODO(fxbug.dev/87645): We can support the general case if necessary.
struct PayloadBuffer {
    mapping: Mapping,
    packets_avail: VecDeque<Range<usize>>, // ranges with mapping
}

impl PayloadBuffer {
    /// Create a PayloadBuffer containing `buffer_bytes` / `packet_bytes` packets.
    pub fn new(
        buffer_bytes: usize,
        packet_bytes: usize,
        name: &str,
    ) -> Result<(Self, zx::Vmo), anyhow::Error> {
        // 5.14.6.6.3.1 Device Requirements: Stream Parameters
        // "If the device has an intermediate buffer, its size MUST be no less
        // than the specified buffer_bytes value."
        let cname = CString::new(name)?;
        let vmo = zx::Vmo::create(buffer_bytes as u64).context("failed to allocate VMO")?;
        vmo.set_name(&cname)?;
        let flags = zx::VmarFlags::PERM_READ
            | zx::VmarFlags::PERM_WRITE
            | zx::VmarFlags::MAP_RANGE
            | zx::VmarFlags::REQUIRE_NON_RESIZABLE;
        let mut pb = PayloadBuffer {
            mapping: Mapping::create_from_vmo(&vmo, buffer_bytes, flags)
                .context("failed to create Mapping from VMO")?,
            packets_avail: VecDeque::new(),
        };

        // Initially, all packets are available.
        let num_packets = buffer_bytes / packet_bytes;
        for k in 0..num_packets {
            let offset = k * packet_bytes;
            pb.packets_avail.push_back(Range { start: offset, end: offset + packet_bytes });
        }

        Ok((pb, vmo))
    }
}

/// State common to AudioRenderer and AudioCapturer connections.
/// Borrowed references to this object should not be held across an await.
struct AudioStreamConn<T> {
    fidl_proxy: T,
    params: AudioStreamParams,
    payload_buffer: PayloadBuffer,
    lead_time: tokio::sync::watch::Receiver<zx::Duration>,
    packets_received: u32,
    packets_pending: HashMap<u32, Notification>, // signalled when we're done with the packet
    closing: Notification,                       // signalled when we've started disconnecting
}

impl<T> AudioStreamConn<T> {
    // Returns the current latency of this stream, in bytes.
    fn latency_bytes(&self) -> u32 {
        // bytes_for_duration fails only if the lead_time is negative, which should
        // never happen unless audio_core is buggy.
        let lead_time = *self.lead_time.borrow();
        match wire_convert::bytes_for_duration(lead_time, self.params.stream_type) {
            // The virtio-sound wire protocol encodes latency_bytes with u32. In practice that
            // gives a maximum lead time of 5592s (93 minutes) for a 96kHz mono stream with 4-byte
            // (float) samples. This is more than enough for all practical scenarios. If this cast
            // fails, there's probably a bug.
            Some(bytes) => match num_traits::cast::cast::<usize, u32>(bytes) {
                Some(bytes) => bytes,
                None => {
                    // TODO(fxbug.dev/87645): throttle
                    tracing::warn!(
                        "got unexpectedly large lead time: {}ns ({} bytes)",
                        lead_time.into_nanos(),
                        bytes
                    );
                    u32::max_value()
                }
            },
            None => {
                // TODO(fxbug.dev/87645): throttle
                tracing::warn!("got negative lead time: {}ns", lead_time.into_nanos());
                0
            }
        }
    }
}

type AudioOutputConn = AudioStreamConn<fidl_fuchsia_media::AudioRendererProxy>;
type AudioInputConn = AudioStreamConn<fidl_fuchsia_media::AudioCapturerProxy>;

/// Wraps a connection to a FIDL AudioRenderer.
struct AudioOutput<'a> {
    audio: &'a fidl_fuchsia_media::AudioProxy,
    conn: RefCell<Option<AudioOutputConn>>,
    bg_jobs_recv: RefCell<tokio::sync::mpsc::Receiver<AudioOutputBgJob>>,
    bg_jobs_send: RefCell<tokio::sync::mpsc::Sender<AudioOutputBgJob>>,
}

struct AudioOutputBgJob {
    event_stream: fidl_fuchsia_media::AudioRendererEventStream,
    lead_time: tokio::sync::watch::Sender<zx::Duration>,
}

#[async_trait(?Send)]
impl<'a> AudioStream<'a> for AudioOutput<'a> {
    async fn connect(&self, mut params: AudioStreamParams) -> Result<(), Error> {
        // Allocate a payload buffer.
        let (payload_buffer, payload_vmo) =
            PayloadBuffer::new(params.buffer_bytes, params.period_bytes, "AudioRendererBuffer")
                .context("failed to create payload buffer for AudioRenderer")?;

        // Configure the renderer.
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fidl_fuchsia_media::AudioRendererMarker>()?;
        self.audio.create_audio_renderer(server_end)?;
        let fidl_proxy = client_end.into_proxy()?;
        fidl_proxy.add_payload_buffer(0, payload_vmo)?;
        fidl_proxy.set_usage(fidl_fuchsia_media::AudioRenderUsage::Media)?;
        fidl_proxy.set_pcm_stream_type(&mut params.stream_type)?;
        fidl_proxy.enable_min_lead_time_events(true)?;

        // Start a bg job to watch for lead time changes.
        let (lead_time_send, mut lead_time_recv) =
            tokio::sync::watch::channel(zx::Duration::from_nanos(0));
        if let Err(_) = self
            .bg_jobs_send
            .borrow_mut()
            .send(AudioOutputBgJob {
                event_stream: fidl_proxy.take_event_stream(),
                lead_time: lead_time_send,
            })
            .await
        {
            return Err(anyhow!("AudioOutput could not send BgJob: channel closed"));
        }

        // Wait until the renderer is configured. This is signalled by a non-zero lead time.
        loop {
            let deadline = zx::Duration::from_seconds(5).after_now();
            match lead_time_recv.recv().on_timeout(deadline, || None).await {
                Some(t) => {
                    if t > zx::Duration::from_nanos(0) {
                        break;
                    }
                }
                None => {
                    return Err(anyhow!(
                        "failed to create AudioRenderer: did not receive lead time before timeout"
                    ));
                }
            }
        }

        *self.conn.borrow_mut() = Some(AudioOutputConn {
            fidl_proxy,
            params,
            payload_buffer,
            lead_time: lead_time_recv,
            packets_received: 0,
            packets_pending: HashMap::new(),
            closing: Notification::new(),
        });
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), Error> {
        // 5.14.6.6.5.1 Device Requirements: Stream Release
        // - The device MUST complete all pending I/O messages for the specified stream ID.
        // - The device MUST NOT complete the control request while there are pending I/O
        //   messages for the specified stream ID.
        //
        // To implement this requirement, we set the `closing` notification, which will cause
        // all pending and future on_receive_data calls to fail with an IO_ERR. Then, we wait
        // until all pending on_receive_data calls have completed before disconnecting.
        let futs = match &mut *self.conn.borrow_mut() {
            Some(conn) => {
                conn.closing.set();
                futures::future::join_all(
                    conn.packets_pending.iter().map(|(id, n)| n.clone().when_set()),
                )
            }
            None => panic!("called disconnect() without a connection"),
        };
        futs.await;
        // Writing None here will deallocate all per-connection state, including the
        // FIDL channel and the payload buffer mapping.
        *self.conn.borrow_mut() = None;
        Ok(())
    }

    async fn start(&self) -> Result<(), Error> {
        let fut = match &*self.conn.borrow() {
            Some(conn) => conn
                .fidl_proxy
                .play(fidl_fuchsia_media::NO_TIMESTAMP, fidl_fuchsia_media::NO_TIMESTAMP),
            None => panic!("called start() without a connection"),
        };
        fut.await?;
        Ok(())
    }

    async fn stop(&self) -> Result<(), Error> {
        let fut = match &*self.conn.borrow() {
            Some(conn) => conn.fidl_proxy.pause(),
            None => panic!("called stop() without a connection"),
        };
        fut.await?;
        Ok(())
    }

    async fn on_receive_data<'b, 'c>(
        &self,
        mut chain: ReadableChain<'b, 'c>,
        lock: sequencer::Lock,
    ) -> Result<(), Error> {
        let mut conn_option = self.conn.borrow_mut();
        let conn = match &mut *conn_option {
            Some(conn) => conn,
            None => panic!("called on_receive_data() without a connection"),
        };

        if conn.closing.get() {
            tracing::warn!("AudioOutput received buffer while connection is closing");
            return reply_txq::err(chain, wire::VIRTIO_SND_S_IO_ERR, 0);
        }

        if let Err(err) = self.validate_packet(conn, &chain) {
            tracing::warn!("{}", err);
            return reply_txq::err(chain, wire::VIRTIO_SND_S_BAD_MSG, conn.latency_bytes());
        }

        let packet_size = chain.remaining()?;
        let packet_range = match conn.payload_buffer.packets_avail.pop_front() {
            Some(range) => range,
            None => {
                tracing::warn!(
                    "AudioOutput ran out of packets (latest buffer has size {} bytes, period is {} bytes)",
                    packet_size,
                    conn.params.period_bytes
                );
                return reply_txq::err(chain, wire::VIRTIO_SND_S_IO_ERR, conn.latency_bytes());
            }
        };

        // Always return this packet when we're done.
        scopeguard::defer!(
            // Need to reacquire this borrow since we don't hold it across the await.
            match &mut *self.conn.borrow_mut() {
                Some(conn) => {
                    conn.payload_buffer.packets_avail.push_back(packet_range.clone());
                    ()
                }
                None => (), // ignore: disconnected while before our await completed
            }
        );

        // Copy the data into the packet.
        let mut offset = 0;
        let packet = conn.payload_buffer.mapping.slice_mut(packet_range.clone());

        while let Some(range) = chain.next().transpose()? {
            // This fails only if the buffer is empty, in which case we can ignore the buffer.
            let ptr = match range.try_mut_ptr::<u8>() {
                Some(ptr) => ptr,
                None => continue,
            };

            // Cast to a &[u8], then copy to the packet.
            //
            // SAFETY: The range comes from a chain, so by construction it refers to a valid
            // range of memory and we are appropriately synchronized with a well-behaved driver.
            // `try_ptr_mut` verifies the pointer is correctly aligned. The worst a buggy driver
            // could do is write to this buffer concurrently, causing us to read garbage bytes,
            // which would produce garbage audio at the speaker.
            let buf = unsafe { std::slice::from_raw_parts(ptr, range.len()) };
            offset += packet.write_at(offset, buf);
        }

        // A notification that is signalled when the packet is done.
        let when_done = Notification::new();
        scopeguard::defer!(when_done.set());

        // Add to the pending set.
        let packet_id = conn.packets_received;
        conn.packets_received += 1;
        conn.packets_pending.insert(packet_id, when_done.clone());
        scopeguard::defer!(
            // Need to reacquire this borrow since we don't hold it across the await.
            match &mut *self.conn.borrow_mut() {
                Some(conn) => {
                    conn.packets_pending.remove(&packet_id);
                    ()
                }
                None => (), // ignore: disconnected while before our await completed
            }
        );

        // Send this packet.
        let resp_fut = conn.fidl_proxy.send_packet(&mut fidl_fuchsia_media::StreamPacket {
            pts: fidl_fuchsia_media::NO_TIMESTAMP,
            payload_buffer_id: 0,
            payload_offset: packet_range.start as u64,
            payload_size: offset as u64, // total bytes copied into the packet
            flags: 0,
            buffer_config: 0,
            stream_segment_id: 0,
        });

        // A future notified if the connection starts to close.
        let closing = conn.closing.clone();
        let closing_fut = closing.when_set();

        // Before we await, drop these borrowed refs so that other async tasks can borrow
        // the conn while we're waiting.
        std::mem::drop(packet);
        std::mem::drop(conn);
        std::mem::drop(conn_option);

        // Before we await, release the sequencer lock so that other packets can be processed
        // concurrently.
        std::mem::drop(lock);

        // Wait until the packet is complete or the connection is closed.
        // We wait until the packet is complete so the return of this packet can act
        // as an "elapsed period" notification. See discussion in 5.14.6.8.
        futures::select! {
            resp = resp_fut.fuse() =>
                match &*self.conn.borrow() {
                    Some(conn) => match resp {
                        Ok(_) => {
                            reply_txq::success(chain, conn.latency_bytes())?
                        },
                        Err(err) =>{
                            tracing::warn!("AudioOutput failed to send packet: {}", err);
                            reply_txq::err(chain, wire::VIRTIO_SND_S_IO_ERR, conn.latency_bytes())?
                        },
                    },
                    None => {
                        // Disconnected before the packet completed. We may hit this case instead
                        // of the closing_fut case below because both futures can be ready at the
                        // same time and select! is not deterministic.
                        reply_txq::err(chain, wire::VIRTIO_SND_S_IO_ERR, 0)?
                    },
                },
            _ = closing_fut.fuse() => {
                // Disconnected before the packet completed.
                reply_txq::err(chain, wire::VIRTIO_SND_S_IO_ERR, 0)?
            }
        };

        Ok(())
    }

    async fn do_background_work(&self) -> Result<(), Error> {
        while let Some(mut job) = self.bg_jobs_recv.borrow_mut().recv().await {
            loop {
                futures::select! {
                    event = job.event_stream.try_next().fuse() =>
                        match event {
                            Ok(event) => {
                                use fidl_fuchsia_media::AudioRendererEvent::OnMinLeadTimeChanged;
                                match event {
                                    Some(OnMinLeadTimeChanged { min_lead_time_nsec }) => {
                                        job.lead_time.broadcast(zx::Duration::from_nanos(min_lead_time_nsec));
                                    },
                                    // Should get an end-of-stream Err before Ok(None).
                                    None => tracing::warn!("AudioRenderer event stream: unexpected end-of-stream"),
                                    _ => tracing::warn!("AudioRenderer event stream: unhandled event {:?}", event),
                                }
                            },
                            Err(err) => {
                                match err {
                                    fidl::Error::ClientChannelClosed{..} => (),
                                    _ => tracing::warn!("AudioRenderer event stream: unexpected error: {}", err),
                                }
                                // FIDL connection broken.
                                break;
                            }
                    },
                    _ = job.lead_time.closed().fuse() => {
                        // The AudioOutput has been disconnected.
                        break;
                    },
                }
            }
        }

        Ok(())
    }
}

impl<'a> AudioOutput<'a> {
    fn validate_packet<'b, 'c>(
        &self,
        conn: &AudioOutputConn,
        chain: &ReadableChain<'b, 'c>,
    ) -> Result<(), Error> {
        // See comments at PayloadBuffer: we assume the total size of the chain is no
        // bigger than period_bytes, allowing the chain to fit in one packet.
        let total_size = chain.remaining()?;
        if total_size > conn.params.period_bytes {
            return Err(anyhow!(
                "AudioOutput received buffer with {} bytes, want < {} bytes",
                total_size,
                conn.params.period_bytes
            ));
        }

        // Must have an integral number of frames per packet.
        let frame_bytes = wire_convert::bytes_per_frame(conn.params.stream_type);
        if total_size % frame_bytes != 0 {
            return Err(anyhow!(
                "AudioOutput received buffer with {} bytes, want multiple of frame size ({} bytes)",
                total_size,
                frame_bytes,
            ));
        }

        Ok(())
    }
}

/// Wraps a connection to a FIDL AudioCapturer.
struct AudioInput<'a> {
    audio: &'a fidl_fuchsia_media::AudioProxy,
    conn: RefCell<Option<AudioInputConn>>,
}

// TODO(fxbug.dev/87645): implement
#[async_trait(?Send)]
impl<'a> AudioStream<'a> for AudioInput<'a> {
    async fn connect(&self, _params: AudioStreamParams) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }

    async fn start(&self) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }

    async fn stop(&self) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }

    async fn on_receive_data<'b, 'c>(
        &self,
        _chain: ReadableChain<'b, 'c>,
        _lock: sequencer::Lock,
    ) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }

    async fn do_background_work(&self) -> Result<(), Error> {
        tracing::warn!("AudioInput is not implemented");
        Ok(())
    }
}

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef GARNET_LIB_MEDIA_CAMERA_SIMPLE_CAMERA_LIB_VIDEO_DISPLAY_H_
#define GARNET_LIB_MEDIA_CAMERA_SIMPLE_CAMERA_LIB_VIDEO_DISPLAY_H_

#include <deque>
#include <list>

#include <fbl/vector.h>
#include <fuchsia/images/cpp/fidl.h>
#include <garnet/lib/media/camera/simple_camera_lib/fenced_buffer.h>
#include <garnet/lib/media/camera/simple_camera_lib/frame_scheduler.h>

#include <garnet/drivers/usb_video/usb-video-camera.h>

namespace simple_camera {

using OnShutdownCallback = fit::function<void()>;
using FrameNotifyCallback =
    fit::function<zx_status_t(fuchsia::camera::driver::FrameAvailableEvent)>;

class VideoDisplay {
 public:
  // Connect to a camera with |camera_id|. If the camera exists, and can be
  // connected to, configures the camera to the first available format, and
  // starts streaming data over the image pipe.
  // Returns an error if the initial part of setup fails.  If ZX_OK is
  // returned, termination of communication is signalled by calling |callback|,
  // which may be done on an arbitrary thread.
  zx_status_t ConnectToCamera(
      uint32_t camera_id,
      ::fidl::InterfaceHandle<::fuchsia::images::ImagePipe> image_pipe,
      OnShutdownCallback callback);

  void DisconnectFromCamera();

  ~VideoDisplay() = default;
  VideoDisplay() = default;

 private:
  // Called when the driver tells us a new frame is available:
  zx_status_t IncomingBufferFilled(
      const fuchsia::camera::driver::FrameAvailableEvent& frame);

  // Called to reserve a buffer for writing.  Currently, this is only called
  // by IncomingBufferFilled.  It should be possible to get notified that the
  // frame is being written, and get a pipelining benefit from notifying
  // scenic earlier.  Scenic would have to allow erroneous frames to be
  // cancelled though.
  zx_status_t ReserveIncomingBuffer(FencedBuffer* buffer,
                                    uint64_t capture_time_ns);

  // Called when a buffer is released by the consumer.
  void BufferReleased(FencedBuffer* buffer);

  // Creates a new buffer and registers an image with scenic.  If the buffer
  // already exists, returns a pointer to that buffer.  Buffer is not required
  // to be valid. If it is nullptr, the returned status can be used to check if
  // that buffer is now available.
  // TODO(garratt): There is currently no way to detect overlapping or unused
  // frames to remove them.
  zx_status_t FindOrCreateBuffer(
      uint32_t frame_size, uint64_t vmo_offset, FencedBuffer** buffer,
      const fuchsia::camera::driver::VideoFormat& format);

  zx_status_t OpenCamera(int dev_id);

  zx_status_t OpenFakeCamera();

  // The currently selected format.
  fuchsia::camera::driver::VideoFormat format_;

  // The number of buffers to allocate while setting up the camera stream.
  // This number has to be at least 2, since scenic will hold onto one buffer
  // at all times.
  static constexpr uint16_t kNumberOfBuffers = 8;

  // Image pipe to send to display
  fuchsia::images::ImagePipePtr image_pipe_;

  // Callback to that we're closing communication:
  OnShutdownCallback on_shut_down_callback_ = nullptr;

  std::vector<std::unique_ptr<FencedBuffer>> frame_buffers_;
  uint32_t last_buffer_index_ = 0;
  uint64_t max_frame_size_ = 0;

  zx::vmo vmo_;
  SimpleFrameScheduler frame_scheduler_;

  class CameraClient {
   public:
    fuchsia::camera::driver::ControlSyncPtr control_;
    fuchsia::camera::driver::StreamSyncPtr stream_;
    fuchsia::camera::driver::StreamEventsPtr events_;
  };
  std::unique_ptr<CameraClient> camera_client_;

  FXL_DISALLOW_COPY_AND_ASSIGN(VideoDisplay);
};

}  // namespace simple_camera

#endif  // GARNET_LIB_MEDIA_CAMERA_SIMPLE_CAMERA_LIB_VIDEO_DISPLAY_H_

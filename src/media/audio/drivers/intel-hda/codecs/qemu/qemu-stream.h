// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_INTEL_HDA_CODECS_QEMU_QEMU_STREAM_H_
#define SRC_MEDIA_AUDIO_DRIVERS_INTEL_HDA_CODECS_QEMU_QEMU_STREAM_H_

#include <fbl/ref_ptr.h>
#include <intel-hda/codec-utils/streamconfig-base.h>

namespace audio {
namespace intel_hda {
namespace codecs {

class QemuStream : public IntelHDAStreamConfigBase {
 public:
  QemuStream(uint32_t stream_id, bool is_input, uint16_t converter_nid);

 protected:
  friend class fbl::RefPtr<QemuStream>;

  virtual ~QemuStream() {}

  // IntelHDAStreamBase implementation
  zx_status_t OnActivateLocked() __TA_REQUIRES(obj_lock()) final;
  void OnDeactivateLocked() __TA_REQUIRES(obj_lock()) final;
  zx_status_t BeginChangeStreamFormatLocked(const audio_proto::StreamSetFmtReq& fmt)
      __TA_REQUIRES(obj_lock()) final;
  zx_status_t FinishChangeStreamFormatLocked(uint16_t encoded_fmt) __TA_REQUIRES(obj_lock()) final;
  void OnGetStringLocked(const audio_proto::GetStringReq& req, audio_proto::GetStringResp* out_resp)
      __TA_REQUIRES(obj_lock()) final;

 private:
  zx_status_t RunCmdListLocked(const CodecVerb* list, size_t count, bool force_all = false)
      __TA_REQUIRES(obj_lock());
  zx_status_t DisableConverterLocked(bool force_all = false) __TA_REQUIRES(obj_lock());
  const uint16_t converter_nid_;
};

}  // namespace codecs
}  // namespace intel_hda
}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_INTEL_HDA_CODECS_QEMU_QEMU_STREAM_H_

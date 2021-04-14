// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_VP9_UTILS_H_
#define SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_VP9_UTILS_H_

#include <fuchsia/mediacodec/cpp/fidl.h>
#include <lib/fit/result.h>

#include <cstdint>
#include <vector>

namespace amlogic_decoder {

std::vector<uint32_t> TryParseSuperframeHeader(const uint8_t* data, uint32_t frame_size);
// like_secmem - Fill output_vector with the same data that would be generated by secmem TA's
//     kSecmemCommandIdGetVp9HeaderSize.  When false, output_vector will include only the AMLV
//     headers and the frame data.  When true, output_vector will contain zeroes after the last
//     frame equal in size to the superframe header, if any.  When false, the output_vector will be
//     frame_size + output_vector->size() * 16.
void SplitSuperframe(const uint8_t* data, uint32_t frame_size, std::vector<uint8_t>* output_vector,
                     std::vector<uint32_t>* superframe_byte_sizes = nullptr,
                     bool like_secmem = false);

fit::result<bool, fuchsia::media::StreamError> IsVp9KeyFrame(uint8_t frame_header_byte_0);

const uint32_t kVp9AmlvHeaderSize = 16;
const uint32_t kVp9FrameMarker = 2;
const uint32_t kVp9FrameTypeKeyFrame = 0;
const uint32_t kVp9FrameTypeNonKeyFrame = 1;

}  // namespace amlogic_decoder

#endif  // SRC_MEDIA_DRIVERS_AMLOGIC_DECODER_VP9_UTILS_H_

// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_MESSAGE_BUFFER_H_
#define LIB_FIDL_CPP_MESSAGE_BUFFER_H_

#include <lib/fidl/cpp/message.h>
#include <stdint.h>
#include <zircon/fidl.h>
#include <zircon/types.h>

namespace fidl {

class OutgoingMessageBuffer {
 public:
  // Creates a |MessageBuffer| that allocates buffers for message of the
  // given capacities.
  //
  // The buffers are freed when the |MessageBuffer| is destructed.
  explicit OutgoingMessageBuffer(uint32_t bytes_capacity = ZX_CHANNEL_MAX_MSG_BYTES,
                                 uint32_t handles_capacity = ZX_CHANNEL_MAX_MSG_HANDLES);

  // The memory that backs the message is freed by this destructor.
  ~OutgoingMessageBuffer();

  // The memory in which bytes can be stored in this buffer.
  uint8_t* bytes() const { return buffer_; }

  // The total number of bytes that can be stored in this buffer.
  uint32_t bytes_capacity() const { return bytes_capacity_; }

  // The memory in which handles can be stored in this buffer.
  zx_handle_disposition_t* handles() const;

  // The total number of handles that can be stored in this buffer.
  uint32_t handles_capacity() const { return handles_capacity_; }

  // Creates a |HLCPPOutgoingMessage| that is backed by the memory in this buffer.
  //
  // The returned |HLCPPOutgoingMessage| contains no bytes or handles.
  HLCPPOutgoingMessage CreateEmptyOutgoingMessage();

 private:
  uint8_t* const buffer_;
  const uint32_t bytes_capacity_;
  const uint32_t handles_capacity_;
};

class IncomingMessageBuffer {
 public:
  // Creates a |MessageBuffer| that allocates buffers for message of the
  // given capacities.
  //
  // The buffers are freed when the |MessageBuffer| is destructed.
  explicit IncomingMessageBuffer(uint32_t bytes_capacity = ZX_CHANNEL_MAX_MSG_BYTES,
                                 uint32_t handles_capacity = ZX_CHANNEL_MAX_MSG_HANDLES);

  // The memory that backs the message is freed by this destructor.
  ~IncomingMessageBuffer();

  // The memory in which bytes can be stored in this buffer.
  uint8_t* bytes() const { return buffer_; }

  // The total number of bytes that can be stored in this buffer.
  uint32_t bytes_capacity() const { return bytes_capacity_; }

  // The memory in which handles can be stored in this buffer.
  zx_handle_info_t* handles() const;

  // The total number of handles that can be stored in this buffer.
  uint32_t handles_capacity() const { return handles_capacity_; }

  // Creates a |HLCPPIncomingMessage| that is backed by the memory in this buffer.
  //
  // The returned |HLCPPIncomingMessage| contains no bytes or handles.
  HLCPPIncomingMessage CreateEmptyIncomingMessage();

 private:
  uint8_t* const buffer_;
  const uint32_t bytes_capacity_;
  const uint32_t handles_capacity_;
};

}  // namespace fidl

#endif  // LIB_FIDL_CPP_MESSAGE_BUFFER_H_

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iostream>

#include "lib/syslog/cpp/log_settings.h"
#include "lib/syslog/global.h"
#include "logging_backend_shared.h"
#include "macros.h"

namespace syslog_backend {

bool fx_log_compat_no_interest_listener() { return false; }
bool fx_log_compat_flush_record(LogBuffer* buffer) {
  auto header = MsgHeader::CreatePtr(buffer);
  // Write fatal logs to stderr as well because death tests sometimes verify a certain
  // log message was printed prior to the crash.
  // TODO(samans): Convert tests to not depend on stderr. https://fxbug.dev/49593
  if (header->severity == syslog::LOG_FATAL) {
    std::cerr << header->c_str() << std::endl;
  }
  fx_logger_t* logger = fx_log_get_logger();
  if (header->user_tag) {
    fx_logger_log(logger, header->severity, header->user_tag, header->c_str());
  } else {
    fx_logger_log(logger, header->severity, nullptr, header->c_str());
  }
  return true;
}

}  // namespace syslog_backend

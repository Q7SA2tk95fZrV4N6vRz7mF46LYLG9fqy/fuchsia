// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_PERFMON_CONFIG_IMPL_H_
#define SRC_PERFORMANCE_LIB_PERFMON_CONFIG_IMPL_H_

#include <fuchsia/perfmon/cpu/cpp/fidl.h>

#include "src/performance/lib/perfmon/config.h"

namespace perfmon {

using FidlPerfmonConfig = fuchsia::perfmon::cpu::Config;

namespace internal {

// Convert the config to the FIDL representation.
void PerfmonToFidlConfig(const Config& config, FidlPerfmonConfig* out_config);

}  // namespace internal

}  // namespace perfmon

#endif  // SRC_PERFORMANCE_LIB_PERFMON_CONFIG_IMPL_H_

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_PERFMON_PROPERTIES_IMPL_H_
#define SRC_PERFORMANCE_LIB_PERFMON_PROPERTIES_IMPL_H_

#include <fuchsia/perfmon/cpu/cpp/fidl.h>
#include <lib/zircon-internal/device/cpu-trace/perf-mon.h>

#include "src/performance/lib/perfmon/properties.h"

namespace perfmon {

using FidlPerfmonProperties = fuchsia::perfmon::cpu::Properties;

namespace internal {

void FidlToPerfmonProperties(const FidlPerfmonProperties& props, Properties* out_props);

}  // namespace internal

}  // namespace perfmon

#endif  // SRC_PERFORMANCE_LIB_PERFMON_PROPERTIES_IMPL_H_

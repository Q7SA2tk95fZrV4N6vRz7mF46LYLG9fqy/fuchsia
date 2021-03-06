// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DEBUGGER_UTILS_BUILD_IDS_H_
#define SRC_LIB_DEBUGGER_UTILS_BUILD_IDS_H_

#include <string>
#include <vector>

#include "src/lib/fxl/macros.h"

namespace debugger_utils {

struct BuildId {
  std::string build_id;
  std::string file;
};

class BuildIdTable {
 public:
  BuildIdTable() = default;

  bool ReadIdsFile(const std::string& file);

  const BuildId* LookupBuildId(const std::string& bid);

 private:
  void AddBuildId(const std::string& file_dir, const std::string& build_id,
                  const std::string& path);

  std::vector<BuildId> build_ids_;

  FXL_DISALLOW_COPY_AND_ASSIGN(BuildIdTable);
};

}  // namespace debugger_utils

#endif  // SRC_LIB_DEBUGGER_UTILS_BUILD_IDS_H_

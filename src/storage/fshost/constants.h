// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_FSHOST_CONSTANTS_H_
#define SRC_STORAGE_FSHOST_CONSTANTS_H_

#include <string_view>

namespace fshost {

// These need to match whatever our imaging tools do.
inline constexpr std::string_view kBlobfsPartitionLabel = "blobfs";
inline constexpr std::string_view kDataPartitionLabel = "data";
inline constexpr std::string_view kLegacyDataPartitionLabel = "minfs";

// This is the path the to fshost admin service that shell tools should see.
inline constexpr std::string_view kHubAdminServicePath =
    "/hub-v2/children/bootstrap/children/fshost/exec/out/svc/fuchsia.fshost.Admin";

}  // namespace fshost

#endif  // SRC_STORAGE_FSHOST_CONSTANTS_H_

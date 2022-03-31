// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "zbi-test-entry.h"

#include <fcntl.h>
#include <lib/fdio/io.h>
#include <lib/zbitl/error-stdio.h>
#include <lib/zbitl/items/bootfs.h>
#include <lib/zbitl/view.h>
#include <lib/zbitl/vmo.h>
#include <lib/zx/vmo.h>
#include <stdio.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <cstddef>
#include <string_view>

#include <fbl/unique_fd.h>
#include <src/bringup/lib/mexec/mexec.h>

constexpr const char* kMexecZbi = "testdata/mexec-child.zbi";

namespace {

using BootfsView = zbitl::BootfsView<zbitl::MapUnownedVmo>;

// The bootfs VFS (rooted under '/boot') is hosted by component manager. These tests can be started
// directly from userboot without starting component manager, so the bootfs VFS will not be
// available. Instead, we can just read any files needed directly from the uncompressed bootfs VMO.
zx_status_t GetFileFromBootfs(std::string_view path, zbitl::MapUnownedVmo bootfs, zx::vmo* vmo) {
  BootfsView view;
  if (auto result = BootfsView::Create(std::move(bootfs)); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
    return ZX_ERR_INTERNAL;
  } else {
    view = std::move(result).value();
  }

  auto file = view.find(path);
  if (auto result = view.take_error(); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
    return ZX_ERR_INTERNAL;
  }
  if (file == view.end()) {
    return ZX_ERR_NOT_FOUND;
  }

  return view.storage().vmo().create_child(ZX_VMO_CHILD_SNAPSHOT | ZX_VMO_CHILD_NO_WRITE,
                                           file->offset, file->size, vmo);
}

}  // namespace

zx::status<> ZbiTestEntry::Init(int argc, char** argv) {
  ZX_ASSERT(argc > 0);
  const char* program_name = argv[0];

  zx::vmo bootfs(zx_take_startup_handle(PA_HND(PA_VMO_BOOTFS, 0)));
  if (!bootfs.is_valid()) {
    printf("%s: received an invalid bootfs VMO handle\n", program_name);
    return zx::error(ZX_ERR_INTERNAL);
  }

  {
    zx::vmo vmo;
    zbitl::MapUnownedVmo unowned_bootfs(bootfs.borrow());
    if (zx_status_t status = GetFileFromBootfs(kMexecZbi, unowned_bootfs, &vmo); status != ZX_OK) {
      printf("%s: failed to get child ZBI's VMO: %s\n", program_name, zx_status_get_string(status));
      return zx::error(status);
    }

    zbitl::View view(std::move(vmo));
    if (view.begin() == view.end()) {
      if (auto result = view.take_error(); result.is_error()) {
        printf("%s: invalid child ZBI: ", program_name);
        zbitl::PrintViewError(result.error_value());
      } else {
        printf("%s: empty child ZBI\n", program_name);
      }
      return zx::error(ZX_ERR_INTERNAL);
    }

    auto first = view.begin();
    auto second = std::next(first);
    if (auto result = view.Copy(first, second); result.is_error()) {
      printf("%s: failed to copy out kernel payload: ", program_name);
      zbitl::PrintViewCopyError(result.error_value());
      view.ignore_error();
      return zx::error(ZX_ERR_INTERNAL);
    } else {
      kernel_zbi_ = std::move(result).value();
    }

    if (auto result = view.Copy(second, view.end()); result.is_error()) {
      printf("%s: failed to copy out data ZBI: ", program_name);
      zbitl::PrintViewCopyError(result.error_value());
      view.ignore_error();
      return zx::error(ZX_ERR_INTERNAL);
    } else {
      data_zbi_ = std::move(result).value();
    }

    if (auto result = view.take_error(); result.is_error()) {
      printf("%s: ZBI iteration failure: ", program_name);
      zbitl::PrintViewError(result.error_value());
      return zx::error(ZX_ERR_INTERNAL);
    }
  }

  root_resource_.reset(zx_take_startup_handle(PA_HND(PA_RESOURCE, 0)));
  if (!root_resource_.is_valid()) {
    printf("%s: unable to get a hold of the root resource\n", program_name);
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx_status_t status = mexec::PrepareDataZbi(root_resource_.borrow(), data_zbi_.borrow());
  if (status != ZX_OK) {
    printf("%s: failed to prepare data ZBI: %s\n", program_name, zx_status_get_string(status));
    return zx::error(status);
  }

  return zx::ok();
}

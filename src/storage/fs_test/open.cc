// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/io/llcpp/fidl.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/llcpp/connect_service.h>
#include <lib/zx/channel.h>

#include <string>

#include <fbl/unique_fd.h>

#include "src/storage/fs_test/fs_test_fixture.h"
#include "src/storage/fs_test/misc.h"

namespace fs_test {
namespace {

namespace fio = fuchsia_io;

using OpenTest = FilesystemTest;

fidl::ClientEnd<fio::Directory> CreateDirectory(uint32_t dir_flags, const std::string& path) {
  EXPECT_EQ(mkdir(path.c_str(), 0755), 0);

  auto endpoints = fidl::CreateEndpoints<fio::Directory>();
  EXPECT_EQ(endpoints.status_value(), ZX_OK);
  EXPECT_EQ(fdio_open(path.c_str(), dir_flags | fio::wire::OPEN_FLAG_DIRECTORY,
                      endpoints->server.TakeChannel().release()),
            ZX_OK);

  return std::move(endpoints->client);
}

zx_status_t OpenFileWithCreate(const fidl::ClientEnd<fio::Directory>& dir,
                               const std::string& path) {
  uint32_t child_flags =
      fio::wire::OPEN_FLAG_CREATE | fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_FLAG_DESCRIBE;
  auto child_endpoints = fidl::CreateEndpoints<fio::Node>();
  EXPECT_EQ(child_endpoints.status_value(), ZX_OK);
  auto open_res = fidl::WireCall(dir).Open(child_flags, fio::wire::MODE_TYPE_FILE,
                                           fidl::StringView::FromExternal(path),
                                           std::move(child_endpoints->server));
  EXPECT_EQ(open_res.status(), ZX_OK);
  auto child = fidl::BindSyncClient(std::move(child_endpoints->client));

  class EventHandler : public fidl::WireSyncEventHandler<fio::Node> {
   public:
    EventHandler() = default;
    zx_status_t status() const { return status_; }

    void OnOpen(fio::Node::OnOpenResponse* event) override { status_ = event->s; }
    zx_status_t Unknown() override { return ZX_ERR_IO; }

   private:
    zx_status_t status_ = ZX_OK;
  };

  EventHandler event_handler;
  auto handle_res = child.HandleOneEvent(event_handler);
  EXPECT_EQ(handle_res.status(), ZX_OK);

  return event_handler.status();
}

TEST_P(OpenTest, OpenFileWithCreateCreatesInReadWriteDir) {
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_RIGHT_WRITABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));
  EXPECT_EQ(OpenFileWithCreate(parent, "b"), ZX_OK);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadOnlyDir) {
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));
  EXPECT_EQ(OpenFileWithCreate(parent, "b"), ZX_ERR_ACCESS_DENIED);
}

TEST_P(OpenTest, OpenFileWithCreateCreatesInReadWriteDirPosixOpen) {
  // OPEN_FLAG_POSIX expands the rights of the connection to be the maximum level of rights
  // available, based on the connection used to call open.
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_RIGHT_WRITABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));

  flags =
      fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_FLAG_POSIX | fio::wire::OPEN_FLAG_DIRECTORY;
  uint32_t mode = fio::wire::MODE_TYPE_DIRECTORY;
  std::string path = ".";
  auto clone_endpoints = fidl::CreateEndpoints<fio::Node>();
  ASSERT_EQ(clone_endpoints.status_value(), ZX_OK);
  auto clone_res = fidl::WireCall(parent).Open(flags, mode, fidl::StringView::FromExternal(path),
                                               std::move(clone_endpoints->server));
  ASSERT_EQ(clone_res.status(), ZX_OK);
  fidl::ClientEnd<fio::Directory> clone_dir(clone_endpoints->client.TakeChannel());

  EXPECT_EQ(OpenFileWithCreate(clone_dir, "b"), ZX_OK);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadOnlyDirPosixOpen) {
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));

  flags =
      fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_FLAG_POSIX | fio::wire::OPEN_FLAG_DIRECTORY;
  uint32_t mode = fio::wire::MODE_TYPE_DIRECTORY;
  std::string path = ".";
  auto clone_endpoints = fidl::CreateEndpoints<fio::Node>();
  ASSERT_EQ(clone_endpoints.status_value(), ZX_OK);
  auto clone_res = fidl::WireCall(parent).Open(flags, mode, fidl::StringView::FromExternal(path),
                                               std::move(clone_endpoints->server));
  ASSERT_EQ(clone_res.status(), ZX_OK);
  fidl::ClientEnd<fio::Directory> clone_dir(clone_endpoints->client.TakeChannel());

  EXPECT_EQ(OpenFileWithCreate(clone_dir, "b"), ZX_ERR_ACCESS_DENIED);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadWriteDirPosixClone) {
  // OPEN_FLAG_POSIX only does the rights expansion with the open call though.
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_RIGHT_WRITABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));

  flags =
      fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_FLAG_POSIX | fio::wire::OPEN_FLAG_DIRECTORY;
  auto clone_endpoints = fidl::CreateEndpoints<fio::Node>();
  ASSERT_EQ(clone_endpoints.status_value(), ZX_OK);
  auto clone_res = fidl::WireCall(parent).Clone(flags, std::move(clone_endpoints->server));
  ASSERT_EQ(clone_res.status(), ZX_OK);
  fidl::ClientEnd<fio::Directory> clone_dir(clone_endpoints->client.TakeChannel());

  EXPECT_EQ(OpenFileWithCreate(clone_dir, "b"), ZX_ERR_ACCESS_DENIED);
}

TEST_P(OpenTest, OpenFileWithCreateFailsInReadOnlyDirPosixClone) {
  uint32_t flags = fio::wire::OPEN_RIGHT_READABLE;
  auto parent = CreateDirectory(flags, GetPath("a"));

  flags =
      fio::wire::OPEN_RIGHT_READABLE | fio::wire::OPEN_FLAG_POSIX | fio::wire::OPEN_FLAG_DIRECTORY;
  auto clone_endpoints = fidl::CreateEndpoints<fio::Node>();
  ASSERT_EQ(clone_endpoints.status_value(), ZX_OK);
  auto clone_res = fidl::WireCall(parent).Clone(flags, std::move(clone_endpoints->server));
  ASSERT_EQ(clone_res.status(), ZX_OK);
  fidl::ClientEnd<fio::Directory> clone_dir(clone_endpoints->client.TakeChannel());

  EXPECT_EQ(OpenFileWithCreate(clone_dir, "b"), ZX_ERR_ACCESS_DENIED);
}

// TODO(fxbug.dev/45624): fix this issue in the rust vfs, then enable these tests for fat.
std::vector<TestFilesystemOptions> GetTestCombinations() {
  return MapAndFilterAllTestFilesystems(
      [](const TestFilesystemOptions& options) -> std::optional<TestFilesystemOptions> {
        if (!options.filesystem->GetTraits().is_fat) {
          return options;
        } else {
          return std::nullopt;
        }
      });
}

INSTANTIATE_TEST_SUITE_P(/*no prefix*/, OpenTest, testing::ValuesIn(GetTestCombinations()),
                         testing::PrintToStringParamName());

}  // namespace
}  // namespace fs_test

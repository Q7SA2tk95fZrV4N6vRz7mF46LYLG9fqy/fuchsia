// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/lib/acpi/client.h"

#include <fuchsia/hardware/acpi/llcpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/fidl-async/cpp/bind.h>

#include <zxtest/zxtest.h>

class MockAcpiDevice : public fidl::WireServer<fuchsia_hardware_acpi::Device> {
 public:
  using EvaluateObjectFn =
      std::function<void(EvaluateObjectRequestView, EvaluateObjectCompleter::Sync &)>;
  explicit MockAcpiDevice(EvaluateObjectFn callback) : evaluate_object_(std::move(callback)) {}

  void GetBusId(GetBusIdRequestView request, GetBusIdCompleter::Sync &completer) override {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
  }

  void EvaluateObject(EvaluateObjectRequestView request,
                      EvaluateObjectCompleter::Sync &completer) override {
    if (evaluate_object_ == nullptr) {
      completer.ReplyError(fuchsia_hardware_acpi::wire::Status::kError);
    } else {
      evaluate_object_(request, completer);
    }
  }

  EvaluateObjectFn evaluate_object_ = nullptr;
};

class AcpiClientTest : public zxtest::Test {
 public:
  AcpiClientTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override { ASSERT_OK(loop_.StartThread("acpi-client-test-thread")); }

 protected:
  async::Loop loop_;
};

TEST_F(AcpiClientTest, TestCallDsm) {
  // Intel NHLT DSM UUID: a69f886e-6ceb-4594-a41f-7b5dce24c553
  const auto uuid = acpi::Uuid::Create(0xa69f886e, 0x6ceb, 0x4594, 0xa41f, 0x7b5dce24c553);
  static constexpr uint8_t kNhltUuid[] = {
      /* 0000 */ 0x6e, 0x88, 0x9f, 0xa6, 0xeb, 0x6c, 0x94, 0x45,
      /* 0008 */ 0xa4, 0x1f, 0x7b, 0x5d, 0xce, 0x24, 0xc5, 0x53};
  ASSERT_BYTES_EQ(uuid.bytes, kNhltUuid, countof(kNhltUuid));

  MockAcpiDevice server([](MockAcpiDevice::EvaluateObjectRequestView request,
                           MockAcpiDevice::EvaluateObjectCompleter::Sync &sync) {
    ASSERT_BYTES_EQ(request->path.data(), "_DSM", request->path.size());
    ASSERT_EQ(request->mode, fuchsia_hardware_acpi::wire::EvaluateObjectMode::kPlainObject);
    ASSERT_EQ(request->parameters.count(), 3);
    auto &params = request->parameters;

    ASSERT_TRUE(params[0].is_buffer_val());
    ASSERT_BYTES_EQ(params[0].buffer_val().data(), kNhltUuid, countof(kNhltUuid));

    ASSERT_TRUE(params[1].is_integer_val());
    ASSERT_EQ(params[1].integer_val(), 1);
    ASSERT_TRUE(params[2].is_integer_val());
    ASSERT_EQ(params[2].integer_val(), 3);

    sync.ReplyError(fuchsia_hardware_acpi::wire::Status::kError);
  });

  auto endpoints = fidl::CreateEndpoints<fuchsia_hardware_acpi::Device>();
  ASSERT_OK(endpoints.status_value());
  fidl::BindSingleInFlightOnly(loop_.dispatcher(), std::move(endpoints->server), &server);

  auto helper = acpi::Client::Create(
      fidl::WireSyncClient<fuchsia_hardware_acpi::Device>(std::move(endpoints->client)));
  auto result = helper.CallDsm(uuid, 1, 3, std::nullopt);
  ASSERT_OK(result.status_value());
}

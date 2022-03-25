// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sys/cpp/component_context.h>

#include "src/sys/fuzzing/libfuzzer/testing/relay.h"

int main(int argc, char** argv) {
  fuzzing::RelayImpl relay;
  auto context = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  context->outgoing()->AddPublicService(relay.GetHandler());
  return relay.Run();
}

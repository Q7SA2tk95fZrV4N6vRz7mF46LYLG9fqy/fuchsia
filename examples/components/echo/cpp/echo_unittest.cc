// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START imports]
#include <gtest/gtest.h>
// [END imports]

#include "examples/components/echo/cpp/echo_component.h"

// [START test_mod]
TEST(EchoTest, TestGreetOne) {
  std::vector<std::string> names = {"Alice"};
  std::string expected = "Alice";
  ASSERT_TRUE(echo::greeting(names) == expected);
}

TEST(EchoTest, TestGreetTwo) {
  std::vector<std::string> names = {"Alice", "Bob"};
  std::string expected = "Alice and Bob";
  ASSERT_TRUE(echo::greeting(names) == expected);
}

TEST(EchoTest, TestGreetThree) {
  std::vector<std::string> names = {"Alice", "Bob", "Spot"};
  std::string expected = "Alice, Bob, Spot";
  ASSERT_TRUE(echo::greeting(names) == expected);
}
// [END test_mod]

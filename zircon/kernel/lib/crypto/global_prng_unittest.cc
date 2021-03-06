// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/crypto/global_prng.h>
#include <lib/unittest/unittest.h>
#include <stdint.h>

namespace crypto {

namespace {
bool identical() {
  BEGIN_TEST;

  Prng* prng1 = global_prng::GetInstance();
  Prng* prng2 = global_prng::GetInstance();

  EXPECT_NE(prng1, nullptr);
  EXPECT_EQ(prng1, prng2);

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(global_prng_tests)
UNITTEST("Identical", identical)
UNITTEST_END_TESTCASE(global_prng_tests, "global_prng", "Validate global PRNG singleton")

}  // namespace crypto

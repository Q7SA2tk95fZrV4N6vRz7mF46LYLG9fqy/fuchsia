// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/lib/processing/coefficient_table.h"

#include <gtest/gtest.h>

#include "src/media/audio/lib/format2/fixed.h"

namespace media_audio {
namespace {

TEST(CoefficientTableTest, AllIndicesAccessible) {
  Fixed width(10);
  CoefficientTableBuilder builder(width.raw_value(), Fixed::Format::FractionalBits);
  for (int64_t i = 0; i < width.raw_value(); ++i) {
    builder[i] = static_cast<float>(i);
  }
  for (int64_t i = 0; i < width.raw_value(); ++i) {
    ASSERT_FLOAT_EQ(builder[i], static_cast<float>(i)) << "index " << i;
  }

  auto table = builder.Build();
  for (int64_t i = 0; i < width.raw_value(); ++i) {
    ASSERT_FLOAT_EQ((*table)[i], static_cast<float>(i)) << "index " << i;
  }
}

TEST(CoefficientTableTest, IntegralStrideHasPhysicallyContiguousIndicies) {
  Fixed width(10);
  CoefficientTable table(width.raw_value(), Fixed::Format::FractionalBits,
                         cpp20::span<const float>{});

  for (int64_t fraction = 0; fraction < kFracOneFrame; ++fraction) {
    // Each fractional value will have a block in the vector. Now check that every valid integral
    // value is contiguous for this fractional value.
    auto block_index = fraction * width.Ceiling();
    for (int64_t integer = 0; integer < width.Ceiling(); ++integer) {
      auto fixed_value = (integer << Fixed::Format::FractionalBits) + fraction;
      ASSERT_EQ(block_index + integer, static_cast<int64_t>(table.PhysicalIndex(fixed_value)))
          << "block_index = " << block_index << ", fixed_value = " << fixed_value;
    }
  }
}

TEST(CoefficientTableTest, ReadSlice) {
  Fixed width(10);
  CoefficientTableBuilder builder(width.raw_value(), Fixed::Format::FractionalBits);

  for (size_t k = 0; k < builder.size(); k++) {
    builder[k] = static_cast<float>(k);
  }

  auto table = builder.Build();
  for (int64_t fraction = 0; fraction < kFracOneFrame; ++fraction) {
    auto slice = table->ReadSlice(fraction, width.Ceiling());
    ASSERT_NE(slice, nullptr);

    for (int64_t i = 0; i < width.Ceiling(); ++i) {
      ASSERT_EQ(slice[i], (*table)[fraction + (i << Fixed::Format::FractionalBits)]);
    }
  }
}

}  // namespace
}  // namespace media_audio

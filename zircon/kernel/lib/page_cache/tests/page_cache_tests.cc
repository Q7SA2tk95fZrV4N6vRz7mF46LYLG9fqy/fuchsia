// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/page_cache.h>
#include <lib/unittest/unittest.h>
#include <zircon/listnode.h>

#include <arch/ops.h>
#include <kernel/auto_preempt_disabler.h>

namespace {

bool page_cache_tests() {
  BEGIN_TEST;

  const size_t reserve_pages = 8;
  auto page_cache_result = page_cache::PageCache::Create(reserve_pages);
  ASSERT_TRUE(page_cache_result.is_ok());

  page_cache::PageCache page_cache = ktl::move(page_cache_result.value());
  EXPECT_EQ(reserve_pages, page_cache.reserve_pages());

  // An allocation from an empty or insufficient page cache fills the cache AND
  // returns the pages requested.
  {
    const size_t page_count = reserve_pages / 2;
    auto result = page_cache.Allocate(page_count);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(page_count, list_length(&result->page_list));
    EXPECT_EQ(reserve_pages, result->available_pages);
  }

  // An allocation from a sufficient page cache does not fill the cache AND
  // reduces the number of pages available.
  {
    const size_t page_count = reserve_pages / 2;
    auto result = page_cache.Allocate(page_count);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(page_count, list_length(&result->page_list));
    EXPECT_EQ(page_count, result->available_pages);
  }

  // An allocation that is too large for the page cache fills the cache AND
  // returns the pages requested.
  {
    const size_t page_count = reserve_pages * 2;
    auto result = page_cache.Allocate(page_count);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(page_count, list_length(&result->page_list));
    EXPECT_EQ(reserve_pages, result->available_pages);
  }

  // Exercise basic free.
  {
    AutoPreemptDisabler<APDInitialState::PREEMPT_DISABLED> preempt_disable;
    const cpu_num_t original_cpu = arch_curr_cpu_num();

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    const size_t page_count = reserve_pages / 2;
    auto result = page_cache.Allocate(page_count);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(page_count, list_length(&result->page_list));
    EXPECT_EQ(reserve_pages - page_count, result->available_pages);

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    page_cache.Free(ktl::move(result->page_list));
    EXPECT_EQ(0u, list_length(&result->page_list));

    auto null_result = page_cache.Allocate(0);
    EXPECT_EQ(reserve_pages, null_result->available_pages);
  }

  // Exercise intermixing small and oversized allocations and frees.
  {
    AutoPreemptDisabler<APDInitialState::PREEMPT_DISABLED> preempt_disable;
    const cpu_num_t original_cpu = arch_curr_cpu_num();

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    const size_t large_page_count = reserve_pages * 2;
    auto large_result = page_cache.Allocate(large_page_count);
    ASSERT_TRUE(large_result.is_ok());
    EXPECT_EQ(large_page_count, list_length(&large_result->page_list));
    EXPECT_EQ(reserve_pages, large_result->available_pages);

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    const size_t page_count = 1;
    auto result = page_cache.Allocate(page_count);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(page_count, list_length(&result->page_list));
    EXPECT_EQ(reserve_pages - page_count, result->available_pages);

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    page_cache.Free(ktl::move(large_result->page_list));
    EXPECT_EQ(0u, list_length(&large_result->page_list));

    auto null_result = page_cache.Allocate(0);
    EXPECT_EQ(reserve_pages, null_result->available_pages);
    EXPECT_EQ(0u, list_length(&null_result->page_list));

    ASSERT_EQ(original_cpu, arch_curr_cpu_num());

    page_cache.Free(ktl::move(result->page_list));
    EXPECT_EQ(0u, list_length(&result->page_list));

    auto null_result2 = page_cache.Allocate(0);
    EXPECT_EQ(reserve_pages, null_result2->available_pages);
    EXPECT_EQ(0u, list_length(&null_result2->page_list));
  }

  END_TEST;
}

}  // anonymous namespace

UNITTEST_START_TESTCASE(page_cache_tests)
UNITTEST("page_cache_tests", page_cache_tests)
UNITTEST_END_TESTCASE(page_cache_tests, "page_cache", "page_cache tests")

// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <arch.h>
#include <debug.h>
#include <lib/console.h>
#include <platform.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <fbl/algorithm.h>
#include <ktl/iterator.h>
#include <pretty/hexdump.h>
#include <vm/pmm.h>
#include <vm/vm_aspace.h>

#include "tests.h"

#include <ktl/enforce.h>

static void mem_test_fail(void* ptr, uint32_t should, uint32_t is) {
  printf("ERROR at %p: should be 0x%x, is 0x%x\n", ptr, should, is);

  ptr = (void*)ROUNDDOWN((uintptr_t)ptr, 64);
  hexdump(ptr, 128);
}

static zx_status_t do_pattern_test(void* ptr, size_t len, uint32_t pat) {
  volatile uint32_t* vbuf32 = reinterpret_cast<volatile uint32_t*>(ptr);
  size_t i;

  printf("\tpattern 0x%08x\n", pat);
  for (i = 0; i < len / 4; i++) {
    vbuf32[i] = pat;
  }

  for (i = 0; i < len / 4; i++) {
    if (vbuf32[i] != pat) {
      mem_test_fail((void*)&vbuf32[i], pat, vbuf32[i]);
      return ZX_ERR_INTERNAL;
    }
  }

  return ZX_OK;
}

static zx_status_t do_moving_inversion_test(void* ptr, size_t len, uint32_t pat) {
  volatile uint32_t* vbuf32 = reinterpret_cast<volatile uint32_t*>(ptr);
  size_t i;

  printf("\tpattern 0x%08x\n", pat);

  /* fill memory */
  for (i = 0; i < len / 4; i++) {
    vbuf32[i] = pat;
  }

  /* from the bottom, walk through each cell, inverting the value */
  // printf("\t\tbottom up invert\n");
  for (i = 0; i < len / 4; i++) {
    if (vbuf32[i] != pat) {
      mem_test_fail((void*)&vbuf32[i], pat, vbuf32[i]);
      return ZX_ERR_INTERNAL;
    }

    vbuf32[i] = ~pat;
  }

  /* repeat, walking from top down */
  // printf("\t\ttop down invert\n");
  for (i = len / 4; i > 0; i--) {
    if (vbuf32[i - 1] != ~pat) {
      mem_test_fail((void*)&vbuf32[i - 1], ~pat, vbuf32[i - 1]);
      return ZX_ERR_INTERNAL;
    }

    vbuf32[i - 1] = pat;
  }

  /* verify that we have the original pattern */
  // printf("\t\tfinal test\n");
  for (i = 0; i < len / 4; i++) {
    if (vbuf32[i] != pat) {
      mem_test_fail((void*)&vbuf32[i], pat, vbuf32[i]);
      return ZX_ERR_INTERNAL;
    }
  }

  return ZX_OK;
}

static void do_mem_tests(void* ptr, size_t len) {
  size_t i;

  /* test 1: simple write address to memory, read back */
  printf("test 1: simple address write, read back\n");
  volatile uint32_t* vbuf32 = reinterpret_cast<volatile uint32_t*>(ptr);
  for (i = 0; i < len / 4; i++) {
    vbuf32[i] = static_cast<uint32_t>(i);
  }

  for (i = 0; i < len / 4; i++) {
    if (vbuf32[i] != i) {
      mem_test_fail((void*)&vbuf32[i], static_cast<uint32_t>(i), vbuf32[i]);
      goto out;
    }
  }

  /* test 2: write various patterns, read back */
  printf("test 2: write patterns, read back\n");

  static const uint32_t patterns[] = {
      0x0,
      0xffffffff,
      0xaaaaaaaa,
      0x55555555,
  };

  for (uint32_t pattern : patterns) {
    if (do_pattern_test(ptr, len, pattern) < 0)
      goto out;
  }
  // shift bits through 32bit word
  for (uint32_t p = 1; p != 0; p <<= 1) {
    if (do_pattern_test(ptr, len, p) < 0)
      goto out;
  }
  // shift bits through 16bit word, invert top of 32bit
  for (uint16_t p = 1; p != 0; p = static_cast<uint16_t>(p << 1)) {
    if (do_pattern_test(ptr, len, ((~p) << 16) | p) < 0)
      goto out;
  }

  /* test 3: moving inversion, patterns */
  printf("test 3: moving inversions with patterns\n");
  for (uint32_t pattern : patterns) {
    if (do_moving_inversion_test(ptr, len, pattern) < 0)
      goto out;
  }
  // shift bits through 32bit word
  for (uint32_t p = 1; p != 0; p <<= 1) {
    if (do_moving_inversion_test(ptr, len, p) < 0)
      goto out;
  }
  // shift bits through 16bit word, invert top of 32bit
  for (uint16_t p = 1; p != 0; p = static_cast<uint16_t>(p << 1)) {
    if (do_moving_inversion_test(ptr, len, ((~p) << 16) | p) < 0)
      goto out;
  }

out:
  printf("done with tests\n");
}

static int mem_test(int argc, const cmd_args* argv, uint32_t flags) {
  if (argc < 2) {
    printf("not enough arguments\n");
  usage:
    printf("usage: %s <length>\n", argv[0].str);
    printf("usage: %s <base> <length>\n", argv[0].str);
    return -1;
  }

  if (argc == 2) {
    void* ptr;
    size_t len = argv[1].u;

    /* rounding up len to the next page */
    len = PAGE_ALIGN(len);
    if (len == 0) {
      printf("invalid length\n");
      return -1;
    }

    /* allocate a region to test in */
    zx_status_t status = VmAspace::kernel_aspace()->AllocContiguous(
        "memtest", len, &ptr, 0, VmAspace::VMM_FLAG_COMMIT,
        ARCH_MMU_FLAG_UNCACHED | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE);
    if (status != ZX_OK) {
      printf("error %d allocating test region\n", status);
      return -1;
    }

    paddr_t pa;
    pa = vaddr_to_paddr(ptr);
    printf("physical address 0x%lx\n", pa);

    printf("got buffer at %p of length 0x%lx\n", ptr, len);

    /* run the tests */
    do_mem_tests(ptr, len);

    /* free the test memory */
    VmAspace::kernel_aspace()->FreeRegion(reinterpret_cast<vaddr_t>(ptr));
  } else if (argc == 3) {
    void* ptr = argv[1].p;
    size_t len = argv[2].u;

    /* run the tests */
    do_mem_tests(ptr, len);
  } else {
    goto usage;
  }

  return 0;
}

STATIC_COMMAND_START
STATIC_COMMAND("mem_test", "test memory", &mem_test)
STATIC_COMMAND_END(mem_tests)

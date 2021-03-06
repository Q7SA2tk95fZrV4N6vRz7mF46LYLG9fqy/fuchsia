// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/errors.h>
#include <zircon/types.h>

#ifndef ZIRCON_KERNEL_ARCH_ARM64_USER_COPY_USER_COPY_H_
#define ZIRCON_KERNEL_ARCH_ARM64_USER_COPY_USER_COPY_H_

// Typically we would not use structs as function return values, but in this case it enables us to
// very efficiently use the 2 registers for return values to encode the optional flags and va
// page fault values.
struct Arm64UserCopyRet {
  zx_status_t status;
  unsigned int pf_flags;
  zx_vaddr_t pf_va;
};
static_assert(sizeof(Arm64UserCopyRet) == 16, "Arm64UserCopyRet has unexpected size");

#ifndef ARM64_USERCOPY_FN
#define ARM64_USERCOPY_FN _arm64_user_copy
#endif

// This is the same as memcpy, except that it takes the additional argument of
// &current_thread()->arch.data_fault_resume, where it temporarily stores the fault recovery PC for
// bad page faults to user addresses during the call, and a fault_return_mask. If
// ARM64_USER_COPY_CAPTURE_FAULTS is passed as fault_return_mask then the returned struct will have
// pf_flags and pf_va filled out on pagefault, otherwise they should be ignored. arch_copy_from_user
// and arch_copy_to_user should be the only callers of this.
extern "C" Arm64UserCopyRet ARM64_USERCOPY_FN(void* dst, const void* src, size_t len,
                                              uint64_t* fault_return, uint64_t fault_return_mask);
#endif  // ZIRCON_KERNEL_ARCH_ARM64_USER_COPY_USER_COPY_H_

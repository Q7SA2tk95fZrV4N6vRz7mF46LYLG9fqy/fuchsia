#include <zircon/process.h>

#include "threads_impl.h"

int __pthread_join(pthread_t t, void** res) {
  switch (zxr_thread_join(&t->zxr_thread)) {
    case ZX_OK:
      __thread_list_erase(t);
      if (res)
        *res = t->start_arg_or_result;
      _zx_vmar_unmap(_zx_vmar_root_self(), (uintptr_t)t->tcb_region.iov_base,
                     t->tcb_region.iov_len);
      return 0;
    default:
      return EINVAL;
  }
}

weak_alias(__pthread_join, pthread_join);

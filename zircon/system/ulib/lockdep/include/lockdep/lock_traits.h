// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#pragma once

#include <stddef.h>
#include <stdint.h>

#include <type_traits>

namespace lockdep {

// Flags to specify which rules to apply to a lock class during validation.
enum LockFlags : uint16_t {
  // Apply only common rules that apply to all locks.
  LockFlagsNone = 0,

  // Apply the irq-safety rules in addition to the common rules for all locks.
  LockFlagsIrqSafe = (1 << 0),

  // Apply the nestable rules in addition to the common rules for all locks.
  LockFlagsNestable = (1 << 1),

  // Apply the multi-acquire rules in additioon to the common rules for all
  // locks.
  LockFlagsMultiAcquire = (1 << 2),

  // Apply the leaf lock rules in addition to the common rules for all locks.
  LockFlagsLeaf = (1 << 3),

  // Do not report validation errors. This flag prevents recursive validation
  // of locks that are acquired by reporting routines.
  LockFlagsReportingDisabled = (1 << 4),

  // There is only one member of this locks class.
  LockFlagsSingletonLock = (1 << 5),

  // Abort the program with an error if a lock is improperly acquired more
  // than once in the same context.
  LockFlagsReAcquireFatal = (1 << 6),

  // Do not add this acquisition to the active list. This may be required for
  // locks that are used to protect context switching logic.
  LockFlagsActiveListDisabled = (1 << 7),

  // Do not track this lock.
  LockFlagsTrackingDisabled = (1 << 8),
};

// Prevent implicit conversion to int of bitwise-or expressions involving
// LockFlags.
constexpr inline LockFlags operator|(LockFlags a, LockFlags b) {
  using U = std::underlying_type_t<LockFlags>;
  return static_cast<LockFlags>(static_cast<U>(a) | static_cast<U>(b));
}

// Prevent implicit conversion to int of bitwise-and expressions involving
// LockFlags.
constexpr inline LockFlags operator&(LockFlags a, LockFlags b) {
  using U = std::underlying_type_t<LockFlags>;
  return static_cast<LockFlags>(static_cast<U>(a) & static_cast<U>(b));
}

namespace internal {

// Receives and validates optional lock flags in the marcos below.
template <LockFlags flags = LockFlagsNone>
struct DefaultLockFlags {
  static constexpr bool IsMultiAcquire = flags & LockFlagsMultiAcquire;
  static constexpr bool IsNestable = flags & LockFlagsNestable;
  static constexpr bool IsReAcquireFatal = flags & LockFlagsReAcquireFatal;

  static_assert(!(IsMultiAcquire && IsNestable),
                "LockFlagsMultiAcquire and LockFlagsNestable are mutually exclusive!");
  static_assert(!(IsMultiAcquire && IsReAcquireFatal),
                "LockFlagsMultiAcquire and LockFlagsReAcquireFatal are mutually exclusive!");

  static constexpr LockFlags value = flags;
};

// Receives and validates the optional lock flags in the macros below and
// injects the singleton lock flag.
template <LockFlags flags = LockFlagsNone>
struct SingletonLockFlags {
  static constexpr bool IsMultiAcquire = flags & LockFlagsMultiAcquire;
  static constexpr bool IsNestable = flags & LockFlagsNestable;

  static_assert(!IsMultiAcquire, "LockFlagsMultiAcquire may not be used with a singleton lock!");
  static_assert(!IsNestable, "LockFlagsNestable may not be used with a singleton lock!");

  static constexpr LockFlags value = flags | LockFlagsSingletonLock;
};

}  // namespace internal

// Forward declaration.
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
class LockDep;

//
// The following macros are used to instrument locks and lock types for runtime
// tracking and validation.
//

// Instruments a lock with dependency tracking features. Instrumentation is
// enabled and disabled by the LOCK_DEP_ENABLE_VALIDATION define.
//
// Arguments:
//  containing_type: The name of the type that contains the lock.
//  lock_type:       The type of the lock to use under the hood.
//
// Example usage:
//
//  struct MyType {
//      LOCK_DEP_INSTRUMENT(MyType, Mutex) mutex;
//      // ...
//  };
#define LOCK_DEP_INSTRUMENT(containing_type, lock_type, ...) \
  ::lockdep::LockDep<containing_type, lock_type, __LINE__,   \
                     ::lockdep::internal::DefaultLockFlags<__VA_ARGS__>::value>

// Defines a singleton lock with the given name and type. The singleton
// instance may be retrieved using the static Get() method provided by the base
// class. This instance is appropriate to pass to Guard<lock_type, [option]>.
//
// Arguments:
//  name:        The name of the singleton to define.
//  lock_type:   The type of the lock to use under the hood.
//  __VA_ARGS__: LockFlags expression specifying lock flags to honor in addition
//               to the flags specified for the lock type using the
//               LOCK_DEP_TRAITS macro below.
//
// Example usage:
//
//  LOCK_DEP_SINGLETON_LOCK(FooLock, fbl::Mutex [, LockFlags]);
//
//  struct Foo { ... }; // Data type of global data.
//
//  Foo g_foo{}; // Global data.
//
//  void MutateFoo() {
//      Guard<fbl::Mutex> guard{FooLock::Get()};
//      // Mutate g_foo ...
//  }
#define LOCK_DEP_SINGLETON_LOCK(name, lock_type, ...)                                              \
  struct name                                                                                      \
      : ::lockdep::SingletonLockDep<name, lock_type,                                               \
                                    ::lockdep::internal::SingletonLockFlags<__VA_ARGS__>::value> { \
  }

// Defines a singleton lock with the given name that wraps a raw global lock.
// The singleton behaves similarly to the version above, except the raw global
// lock is used as the underlying lock instead of an internally-defined lock.
// This is useful to instrument an existing global lock that may be shared with
// C code or for other reasons cannot be completely replaced with the above
// global lock type.
//
// Arguments:
//  name:        The name of the singleton to define.
//  global_lock: The global lock to wrap with this lock type. This must be an
//               lvalue reference expression to a lock with static storage
//               duration, with either external or internal linkage.
//  __VA_ARGS__: LockFlags expression specifying lock flags to honor in addition
//               to the flags specified for the lock type using the
//               LOCK_DEP_TRAITS macro below.
//
// Example usage:
//
//  extern spin_lock_t thread_lock;
//  LOCK_DEP_SINGLETON_LOCK_WRAPPER(ThreadLock, thread_lock [, LockFlags]);
//
//  void DoThreadStuff() {
//      Guard<spin_lock_t, IrqSave> guard{ThreadLock::Get()};
//      // ...
//  }
#define LOCK_DEP_SINGLETON_LOCK_WRAPPER(name, global_lock, ...)                                \
  struct name : public ::lockdep::SingletonLockDep<                                            \
                    name, ::lockdep::GlobalReference<decltype(global_lock), global_lock>,      \
                    ::lockdep::internal::SingletonLockFlags<__VA_ARGS__>::value> {             \
    auto& capability() __TA_RETURN_CAPABILITY(global_lock) { return global_lock; }             \
    const auto& capability() const __TA_RETURN_CAPABILITY(global_lock) { return global_lock; } \
  }

// Tags the given lock type with the given lock state flags value. This informs
// the validator about the properties of the lock to enforce during validation.
// Untagged lock types default to flags lockdep::LockFlagsNone.
//
// This macro must be called once in the same namespace as lock_type is defined
// in and visible to each invocation of LOCK_DEP_INSTRUMENT() for the same lock
// type. If possible, it is a good idea to call the macro close to the
// definition of the lock type.
//
// Arguments:
//  lock_type:  The lock type to specify the lock traits for.
//  lock_flags: A combination of lockdep::LockFlags values that specifies the
//              properties to enforce for the lock type.
//
// Example usage:
//
//  namespace MyNamespace {
//
//  class MySpinLock {
//      // ...
//  };
//  LOCK_DEP_TRAITS(MySpinLock, lockdep::LockFlagsIrqSafe [ | other bits...]);
//
//  }  // MyNamespace
//
#define LOCK_DEP_TRAITS(lock_type, lock_flags)                  \
  template <typename>                                           \
  struct LOCK_DEP_TRAITS;                                       \
  template <>                                                   \
  struct LOCK_DEP_TRAITS<lock_type> {                           \
    static constexpr ::lockdep::LockFlags Flags = (lock_flags); \
  };                                                            \
  LOCK_DEP_TRAITS<lock_type> LOCK_DEP_GetLockTraits(lock_type*)

}  // namespace lockdep

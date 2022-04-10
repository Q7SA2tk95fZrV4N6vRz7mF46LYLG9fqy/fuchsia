// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_
#define ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_

#include <stdint.h>
#include <stdio.h>

#include <ktl/byte.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <phys/main.h>

class FramePointer;
class ShadowCallStackBacktrace;
struct PhysExceptionState;
class Symbolize;

// The Symbolize instance registered by MainSymbolize.
extern Symbolize* gSymbolize;

// Returns the name of the current program, according to the currently
// registered Symbolize object. If no Symbolize has yet been registered, then
// it is assumed that we were in an early set-up context before we have had a
// chance to construct one; in that case, "early-init" is returned.
const char* ProgramName();

class Symbolize {
 public:
  Symbolize() = delete;
  Symbolize(const Symbolize&) = delete;

  explicit Symbolize(const char* name, FILE* f = stdout) : name_(name), output_(f) {}

  const char* name() const { return name_; }

  void set_output(FILE* f) { output_ = f; }

  // Return the hex string for the program's own build ID.
  ktl::string_view BuildIdString();

  // Return the raw bytes for the program's own build ID.
  ktl::span<const ktl::byte> BuildId() const;

  // Print the contextual markup elements describing this phys executable.
  void ContextAlways();

  // Same, but idempotent: the first call prints and others do nothing.
  void Context();

  // Print the presentation markup element for one frame of a backtrace.
  void BackTraceFrame(unsigned int n, uintptr_t pc, bool interrupt = false);

  // Print a backtrace, ensuring context has been printed beforehand.
  // This takes any container of uintptr_t, so FramePointer works.
  template <typename T>
  PHYS_SINGLETHREAD void BackTrace(const T& pcs, unsigned int n = 0) {
    Context();
    for (uintptr_t pc : pcs) {
      BackTraceFrame(n++, pc);
    }
  }

  // Print both flavors of backtrace together.
  PHYS_SINGLETHREAD void PrintBacktraces(const FramePointer& frame_pointers,
                                         const ShadowCallStackBacktrace& shadow_call_stack,
                                         unsigned int n = 0);

  // Print the trigger markup element for a dumpfile.
  // TODO(mcgrathr): corresponds to a ZBI item
  void DumpFile(ktl::string_view type, ktl::string_view name, ktl::string_view desc,
                size_t size_bytes);

  // Dump some stack up to the SP.
  PHYS_SINGLETHREAD void PrintStack(uintptr_t sp,
                                    ktl::optional<size_t> max_size_bytes = ktl::nullopt);

  // Print out register values.
  PHYS_SINGLETHREAD void PrintRegisters(const PhysExceptionState& regs);

  // Print out useful details at an exception.
  PHYS_SINGLETHREAD void PrintException(uint64_t vector, const char* vector_name,
                                        const PhysExceptionState& regs);

 private:
  const char* name_;
  FILE* output_;
  bool context_done_ = false;

  void Printf(const char* fmt, ...);

  // Implementation details of ContextAlways().
  void PrintModule();
  void PrintMmap();
};

// MainSymbolize represents the singleton Symbolize instance to be used by the
// current program. On construction, it regsters itself as `gSymbolize` and
// emits symbolization markup context.
class MainSymbolize : public Symbolize {
 public:
  explicit MainSymbolize(const char* name);
};

#endif  // ZIRCON_KERNEL_PHYS_INCLUDE_PHYS_SYMBOLIZE_H_

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DYNAMIC_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DYNAMIC_H_

#include <lib/stdcompat/span.h>

#include <optional>
#include <string_view>
#include <type_traits>
#include <utility>

#include "internal/dynamic-tag-error.h"
#include "layout.h"
#include "symbol.h"

namespace elfldltl {

using namespace std::literals::string_view_literals;

// elfldltl::DecodeDynamic does a single-pass decoding of the PT_DYNAMIC data
// by statically combining multiple "observer" objects.  Various observers are
// defined to collect different subsets of the dynamic linking metadata needed
// for different purposes.

// This represents a PT_DYNAMIC entry that's been matched to a specific tag.
// These types are used in the arguments to Observe callbacks; see below.
template <ElfDynTag Tag>
struct DynamicTagMatch {};

// This is the base class for Dynamic*Observer classes.
// Each Observer subclass should define these two methods:
//  * template <class DiagnosticsType, class Memory>
//    bool Observe(DiagnosticsType& diagnostics, Memory& memory,
//                 DynamicTagMatch<Tag> tag, Addr val);
//  * template <class DiagnosticsType, class Memory>
//    bool Finish(DiagnosticsType& diagnostics, Memory& memory);
// Observe will be called with each entry matching any Tag of the Tags... list.
// Then Finish will be called at the end of all entries unless processing was
// terminated early for some reason, in which case the observer object is
// usually going to be destroyed without checking its results.  Both return
// false if processing the dynamic section should be terminated early.

template <class Observer, ElfDynTag... Tags>
struct DynamicTagObserver {};

// This decodes a dynamic section by matching each entry against a list of
// observers.  Each observer should be of a subclass of DynamicTagObserver that
// indicates the tags it matches.  If any matching observer returns false then
// this stops processing early and returns false.  Otherwise, each observer's
// Finish method is called, stopping early if one returns false.
template <class DiagnosticsType, class Memory, class Dyn, class... Observers>
constexpr bool DecodeDynamic(DiagnosticsType&& diagnostics, Memory&& memory,
                             cpp20::span<const Dyn> dyn, Observers&&... observers) {
  // The span is an upper bound but the section is terminated by a null entry.
  for (const auto& entry : dyn) {
    // At the terminator entry, call each observer's Finish() method.
    if (entry.tag == ElfDynTag::kNull) {
      return (observers.Finish(diagnostics, memory) && ...);
    }

    // Present each entry to each matching observer (see below).
    if ((!DecodeDynamic(diagnostics, memory, entry, observers) || ...)) {
      return false;
    }
  }

  // This should never be reached.
  return diagnostics.FormatError("missing DT_NULL terminator in PT_DYNAMIC"sv);
}

// Match a single dynamic section entry against a single observer.  If the
// observer matches, its Observe overload for the matching tag is called.
// Returns the value of that call, or true if this observer didn't match.
template <class DiagnosticsType, class Memory, class Dyn, class Observer, ElfDynTag... Tags>
constexpr bool DecodeDynamic(DiagnosticsType&& diagnostics, Memory&& memory, const Dyn& entry,
                             DynamicTagObserver<Observer, Tags...>& observer) {
  bool ok = true;
  auto call_observer = [&](auto dt) {
    ok = static_cast<Observer&>(observer).Observe(diagnostics, memory, dt, entry.val);
    return true;
  };
  ((entry.tag == Tags && call_observer(DynamicTagMatch<Tags>{})) || ...);
  return ok;
}

// This is a very simple observer that rejects DT_TEXTREL.
class DynamicTextrelRejectObserver
    : public DynamicTagObserver<DynamicTextrelRejectObserver, ElfDynTag::kTextRel> {
 public:
  static constexpr std::string_view Message() { return "DT_TEXTREL not supported"sv; }

  template <class DiagnosticsType, class Memory, typename ValueType>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kTextRel> tag, ValueType val) {
    // If this is called at all, that's an error.
    return diagnostics.FormatError(Message());
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Finish(DiagnosticsType& diagnostics, Memory& memory) {
    // There is no state kept aside from in the diagnostics object, so nothing to do.
    return true;
  }
};

// This is a base class for Dynamic*Observer classes in the common pattern
// where an Info object will be filled in with data observed in dynamic
// entries.  A subclass can do `using Base::Base;` to get the constructor that
// takes an Info reference, and has the info() accessor for it.  Then it must
// define its own `Observe` and `Finish` methods.

template <class Observer, class Info, class Elf, ElfDynTag... Tags>
class DynamicInfoObserver : public DynamicTagObserver<Observer, Tags...> {
 public:
  DynamicInfoObserver() = delete;
  constexpr DynamicInfoObserver(const DynamicInfoObserver&) = default;

  constexpr explicit DynamicInfoObserver(Info& info) : info_(info) {}

  constexpr Info& info() { return info_; }
  constexpr const Info& info() const { return info_; }

 protected:
  // This is a utility class for the common pattern of a pair of dynamic tags
  // for a table address and its size in bytes.  The SizedArray object's
  // set_address and set_size_bytes methods should be called from Observe
  // methods for the respective dynamic tags.  Then a Finish method calls
  // SizedArray::Finish<T, Setter, Tag1, Tag2> using explicit template
  // arguments giving the type of table elements, &Info::set_foo for the setter
  // in info() that takes a span<T>, and the two tags, passing it the
  // diagnostics, memory and info objects by reference.  This calls the setter
  // only if both tags were present and the table was successfully fetched from
  // memory.  It diagnoses all the partial and invalid cases in detail with
  // calls to diagnostics.FormatError, and does nothing at all if neither tag
  // is present.
  //
  // The SizedArray object is contextually convertible to bool to test whether
  // either tag was present at all.
  class SizedArray {
   public:
    template <typename T, auto Setter, ElfDynTag AddressTag, ElfDynTag SizeBytesTag,
              class DiagnosticsType, class Memory>
    constexpr bool Finish(DiagnosticsType&& diagnostics, Memory&& memory, Info& info) {
      if (!address_ && !size_bytes_) {
        // No corresponding entries were found.
        return true;
      }
      // Check invariants.
      using Error = internal::DynamicTagError<AddressTag, SizeBytesTag>;
      if (!address_) [[unlikely]] {
        return diagnostics.FormatError(Error::kMissingAddress);
      }
      if (!size_bytes_) [[unlikely]] {
        return diagnostics.FormatError(Error::kMissingSize);
      }
      if (*address_ % alignof(T) != 0) [[unlikely]] {
        // Don't store the bad address so that no misaligned fetches will be
        // attempted later if we keep going to look for more errors.
        *address_ = 0;
        return diagnostics.FormatError(Error::kMisalignedAddress);
      }
      if (*size_bytes_ % sizeof(T) != 0) [[unlikely]] {
        return diagnostics.FormatError(Error::kMisalignedSize);
      }
      // Fetch the table.
      if (auto table = memory.template ReadArray<T>(*address_, *size_bytes_ / sizeof(T))) {
        (info.*Setter)(*table);
        return true;
      }
      return diagnostics.FormatError(Error::kRead);
    }

    constexpr explicit operator bool() const { return address_ || size_bytes_; }

    constexpr void set_address(typename Elf::size_type val) { address_ = val; }

    constexpr void set_size_bytes(typename Elf::size_type val) { size_bytes_ = val; }

   private:
    std::optional<typename Elf::size_type> address_, size_bytes_;
  };

 private:
  Info& info_;
};

// This is an observer to fill in an elfldltl::SymbolInfo<Elf> object.
// Its constructor takes (elfldltl::SymbolInfo<Elf>&, Memory&).
template <class Elf>
class DynamicSymbolInfoObserver;

// This is just a shorthand to avoid repeating the long list of parameters.
template <class Elf>
using DynamicSymbolInfoObserverBase =
    DynamicInfoObserver<DynamicSymbolInfoObserver<Elf>, SymbolInfo<Elf>, Elf, ElfDynTag::kSymTab,
                        ElfDynTag::kSymEnt, ElfDynTag::kHash, ElfDynTag::kGnuHash,
                        ElfDynTag::kStrTab, ElfDynTag::kStrSz, ElfDynTag::kSoname>;

template <class Elf>
class DynamicSymbolInfoObserver : public DynamicSymbolInfoObserverBase<Elf> {
 public:
  using Base = DynamicSymbolInfoObserverBase<Elf>;
  using Info = SymbolInfo<Elf>;
  using size_type = typename Elf::size_type;

  using Base::Base;

  // There's one Observe overload for each dynamic tag the observer handles.

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kStrTab> tag, size_type val) {
    strtab_.set_address(val);
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kStrSz> tag, size_type val) {
    strtab_.set_size_bytes(val);
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kSymTab> tag, size_type val) {
    if (val % sizeof(size_type)) [[unlikely]] {
      // Mark that it was present so we don't diagnose a second error.
      // But don't use a bogus value so no misaligned fetches will be tried.
      symtab_ = 0;
      return diagnostics.FormatError("DT_SYMTAB has misaligned address"sv);
    }
    symtab_ = val;
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kHash> tag, size_type val) {
    if (val % sizeof(uint32_t)) [[unlikely]] {
      return diagnostics.FormatError("DT_HASH has misaligned address"sv);
    }
    hash_ = val;
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kGnuHash> tag, size_type val) {
    if (val % sizeof(size_type)) [[unlikely]] {
      return diagnostics.FormatError("DT_GNU_HASH has misaligned address"sv);
    }
    gnu_hash_ = val;
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kSoname> tag, size_type val) {
    soname_ = val;
    return true;
  }

  template <class DiagnosticsType, class Memory>
  constexpr bool Observe(DiagnosticsType& diagnostics, Memory& memory,
                         DynamicTagMatch<ElfDynTag::kSymEnt> tag, size_type val) {
    return val == sizeof(typename Elf::Sym) ||
           diagnostics.FormatError("incorrect DT_SYMENT value"sv);
  }

  // Check and finalize what's been observed.
  template <class DiagnosticsType, class Memory>
  constexpr bool Finish(DiagnosticsType& diagnostics, Memory& memory) {
    if (hash_) {
      if (auto table = memory.template ReadArray<typename Elf::Word>(*hash_)) {
        this->info().set_compat_hash(*table);
      } else {
        return false;
      }
    }
    if (gnu_hash_) {
      if (auto table = memory.template ReadArray<typename Elf::Addr>(*gnu_hash_)) {
        this->info().set_gnu_hash(*table);
      } else {
        return false;
      }
    }
    if (!symtab_) {
      return !strtab_ || diagnostics.FormatError("DT_STRTAB with no DT_SYMTAB"sv);
    }
    if (auto symtab = memory.template ReadArray<typename Elf::Sym>(*symtab_)) {
      this->info().set_symtab(*symtab);
    } else {
      return false;
    }
    if (!strtab_.template Finish<char, &Info::set_strtab_as_span, ElfDynTag::kStrTab,
                                 ElfDynTag::kStrSz>(diagnostics, memory, this->info())) {
      return false;
    }
    if (soname_) {
      this->info().set_soname(*soname_);
      if (this->info().soname().empty()) [[unlikely]] {
        return diagnostics.FormatError("DT_SONAME does not fit in DT_STRTAB"sv);
      }
    }
    return true;
  }

 private:
  typename Base::SizedArray strtab_;
  std::optional<typename Elf::size_type> symtab_, hash_, gnu_hash_, soname_;
};

// Deduction guide.
template <class Elf>
DynamicSymbolInfoObserver(SymbolInfo<Elf>& info) -> DynamicSymbolInfoObserver<Elf>;

}  // namespace elfldltl

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DYNAMIC_H_

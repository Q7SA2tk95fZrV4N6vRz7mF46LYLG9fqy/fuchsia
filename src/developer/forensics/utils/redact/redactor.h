// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REDACTOR_H_
#define SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REDACTOR_H_

#include <lib/inspect/cpp/vmo/types.h>

#include <string>
#include <string_view>
#include <vector>

#include "src/developer/forensics/utils/redact/cache.h"
#include "src/developer/forensics/utils/redact/replacer.h"

namespace forensics {

class RedactorBase {
 public:
  virtual ~RedactorBase() = default;
  //
  // Redacts |text| in-place and returns a reference to |text|.
  virtual std::string& Redact(std::string& text) = 0;

  // Unredacted / redacted version of canary message for confirming log redaction.
  virtual std::string UnredactedCanary() const = 0;
  virtual std::string RedactedCanary() const = 0;
};

// Redacts PII from text.
//
// TODO(fxbug.dev/94086): keep this class in sync with the Rust redactor until its deleted (located
// in
// https://osscs.corp.google.com/fuchsia/fuchsia/+/main:src/diagnostics/archivist/src/logs/redact.rs
class Redactor : public RedactorBase {
 public:
  explicit Redactor(int starting_id, inspect::UintProperty cache_size);
  ~Redactor() override = default;

  std::string& Redact(std::string& text) override;

  std::string UnredactedCanary() const override;
  std::string RedactedCanary() const override;

 private:
  Redactor& Add(Replacer replacer);
  Redactor& AddTextReplacer(std::string_view pattern, std::string_view replacement);
  Redactor& AddIdReplacer(std::string_view pattern, std::string_view format);

  RedactionIdCache cache_;
  std::vector<Replacer> replacers_;
};

// Do-nothing redactor
class IdentityRedactor : public RedactorBase {
 public:
  ~IdentityRedactor() override = default;

  std::string& Redact(std::string& text) override;

  std::string UnredactedCanary() const override;
  std::string RedactedCanary() const override;
};

}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_UTILS_REDACT_REDACTOR_H_

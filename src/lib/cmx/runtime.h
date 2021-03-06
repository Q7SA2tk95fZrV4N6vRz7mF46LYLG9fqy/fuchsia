// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_CMX_RUNTIME_H_
#define SRC_LIB_CMX_RUNTIME_H_

#include <string>

#include "rapidjson/document.h"
#include "src/lib/json_parser/json_parser.h"

namespace component {

class RuntimeMetadata {
 public:
  // If a config is missing the runtime but otherwise there are no errors,
  // parsing succeeds and IsNull() is true. |json_parser| is used to report any
  // errors.
  void ParseFromFileAt(int dirfd, const std::string& file, json::JSONParser* json_parser);
  void ParseFromDocument(const rapidjson::Document& document, json::JSONParser* json_parser);

  bool IsNull() const { return null_; }
  const std::string& runner() const { return runner_; }

 private:
  bool null_ = true;
  std::string runner_;
};

}  // namespace component

#endif  // SRC_LIB_CMX_RUNTIME_H_

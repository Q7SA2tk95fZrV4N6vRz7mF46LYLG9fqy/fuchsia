// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_ATTACHMENT_MANAGER_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_ATTACHMENT_MANAGER_H_

#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>

#include <memory>

#include "src/developer/forensics/feedback/attachments/provider.h"
#include "src/developer/forensics/feedback/attachments/types.h"

namespace forensics::feedback {

// Responsible for the storage and collection of attachments
//
// Attachments are either static and collected once at startup or dynamic and collected at runtime
// each time they're needed.
class AttachmentManager {
 public:
  explicit AttachmentManager(const std::set<std::string>& allowlist,
                             Attachments static_attachments = {},
                             std::map<std::string, AttachmentProvider*> providers = {});

  ::fpromise::promise<Attachments> GetAttachments(zx::duration timeout);

  void DropStaticAttachment(const AttachmentKey& key, Error error);

 private:
  Attachments static_attachments_;
  std::map<std::string, AttachmentProvider*> providers_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_ATTACHMENTS_ATTACHMENT_MANAGER_H_

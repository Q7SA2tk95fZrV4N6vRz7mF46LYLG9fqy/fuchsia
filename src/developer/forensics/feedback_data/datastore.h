// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_DATASTORE_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_DATASTORE_H_

#include <lib/async/dispatcher.h>
#include <lib/fpromise/promise.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/zx/time.h>

#include <memory>

#include "src/developer/forensics/feedback_data/annotations/annotation_provider.h"
#include "src/developer/forensics/feedback_data/annotations/types.h"
#include "src/developer/forensics/feedback_data/attachments/types.h"
#include "src/developer/forensics/feedback_data/inspect_data_budget.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/fit/timeout.h"
#include "src/developer/forensics/utils/previous_boot_file.h"

namespace forensics {
namespace feedback_data {

// Holds data useful to attach in feedback reports (crash, user feedback or bug reports).
//
// Data can be annotations or attachments.
//
// Some data are:
// * static and collected at startup, e.g., build version or hardware info.
// * dynamic and collected upon data request, e.g., uptime or logs.
// * collected synchronously, e.g., build version or uptime.
// * collected asynchronously, e.g., hardware info or logs.
// * pushed by other components, we called these "non-platform" to distinguish them from the
//   "platform".
//
// Because of dynamic asynchronous data, the data requests can take some time and return a
// ::fpromise::promise.
class Datastore {
 public:
  Datastore(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
            cobalt::Logger* cobalt, const AnnotationKeys& annotation_allowlist,
            const AttachmentKeys& attachment_allowlist, const ErrorOr<std::string>& current_boot_id,
            const ErrorOr<std::string>& previous_boot_id,
            const ErrorOr<std::string>& current_build_version,
            const ErrorOr<std::string>& previous_build_version,
            InspectDataBudget* inspect_data_budget);

  ::fpromise::promise<Annotations> GetAnnotations(zx::duration timeout);
  ::fpromise::promise<Attachments> GetAttachments(zx::duration timeout);

  // Returns whether the non-platform annotations were actually set as there is a cap on the number
  // of non-platform annotations.
  bool TrySetNonPlatformAnnotations(const Annotations& non_platform_annotations);

  // Exposed for testing purposes.
  Datastore(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
            const char* limit_data_flag_path);

  const Annotations& GetStaticAnnotations() const { return static_annotations_; }
  const Attachments& GetStaticAttachments() const { return static_attachments_; }
  const Annotations& GetNonPlatformAnnotations() const { return non_platform_annotations_; }

  bool IsMissingNonPlatformAnnotations() const { return is_missing_non_platform_annotations_; }

  void DropStaticAttachment(const AttachmentKey& key, Error error);

 private:
  ::fpromise::promise<Attachment> BuildAttachment(const AttachmentKey& key, zx::duration timeout);
  ::fpromise::promise<AttachmentValue> BuildAttachmentValue(const AttachmentKey& key,
                                                            zx::duration timeout);
  fit::Timeout MakeCobaltTimeout(cobalt::TimedOutData data, zx::duration timeout);

  async_dispatcher_t* dispatcher_;
  const std::shared_ptr<sys::ServiceDirectory> services_;
  cobalt::Logger* cobalt_;
  const AnnotationKeys annotation_allowlist_;
  AttachmentKeys attachment_allowlist_;

  const Annotations static_annotations_;
  Attachments static_attachments_;

  std::vector<std::unique_ptr<AnnotationProvider>> reusable_annotation_providers_;

  bool is_missing_non_platform_annotations_ = false;
  Annotations non_platform_annotations_;

  InspectDataBudget* inspect_data_budget_;
};

}  // namespace feedback_data
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_DATA_DATASTORE_H_

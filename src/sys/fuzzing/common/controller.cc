// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/common/controller.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/sanitizer.h>
#include <zircon/status.h>

#include "src/sys/fuzzing/common/async-socket.h"
#include "src/sys/fuzzing/common/async-types.h"
#include "src/sys/fuzzing/common/corpus-reader-client.h"
#include "src/sys/fuzzing/common/options.h"

namespace fuzzing {

ControllerImpl::ControllerImpl(ExecutorPtr executor)
    : binding_(this), executor_(std::move(executor)) {
  options_ = MakeOptions();
}

void ControllerImpl::Bind(fidl::InterfaceRequest<Controller> request) {
  FX_DCHECK(runner_);
  binding_.Bind(std::move(request));
}

void ControllerImpl::SetRunner(RunnerPtr runner) {
  runner_ = std::move(runner);
  AddDefaults();
  runner_->Configure(options_);
}

void ControllerImpl::AddDefaults() {
  if (!options_->has_seed()) {
    options_->set_seed(static_cast<uint32_t>(zx::ticks::now().get()));
  }
  runner_->AddDefaults(options_.get());
}

///////////////////////////////////////////////////////////////
// FIDL methods.

void ControllerImpl::Configure(Options options, ConfigureCallback callback) {
  *options_ = std::move(options);
  AddDefaults();
  auto task =
      runner_->Configure(options_).then([callback = std::move(callback)](const ZxResult<>& result) {
        callback(result.is_ok() ? ZX_OK : result.error());
      });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::GetOptions(GetOptionsCallback callback) { callback(CopyOptions(*options_)); }

void ControllerImpl::AddToCorpus(CorpusType corpus_type, FidlInput fidl_input,
                                 AddToCorpusCallback callback) {
  auto task =
      fpromise::make_promise([executor = executor_, fidl_input = std::move(fidl_input)]() mutable {
        return AsyncSocketRead(executor, std::move(fidl_input));
      })
          .and_then([runner = runner_, corpus_type](Input& received) -> ZxResult<> {
            runner->AddToCorpus(corpus_type, std::move(received));
            return fpromise::ok();
          })
          .then([callback = std::move(callback)](const ZxResult<>& result) {
            callback(result.is_ok() ? ZX_OK : result.error());
          });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::ReadCorpus(CorpusType corpus_type, fidl::InterfaceHandle<CorpusReader> reader,
                                ReadCorpusCallback callback) {
  std::vector<Input> inputs;
  for (size_t offset = 1;; ++offset) {
    auto input = runner_->ReadFromCorpus(corpus_type, offset);
    if (input.size() == 0) {
      break;
    }
    inputs.emplace_back(std::move(input));
  }
  auto client = std::make_unique<CorpusReaderClient>(executor_);
  client->Bind(std::move(reader));

  // Prevent the client from going out of scope before the promise completes.
  auto task = fpromise::make_promise(
      [inputs = std::move(inputs), client = std::move(client), callback = std::move(callback),
       sending = ZxFuture<>()](Context& context) mutable -> Result<> {
        if (!sending) {
          sending = client->Send(std::move(inputs));
        }
        if (!sending(context)) {
          return fpromise::pending();
        }
        callback();
        return fpromise::ok();
      });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::WriteDictionary(FidlInput dictionary, WriteDictionaryCallback callback) {
  auto task =
      fpromise::make_promise([executor = executor_, dictionary = std::move(dictionary)]() mutable {
        return AsyncSocketRead(executor, std::move(dictionary));
      })
          .and_then([runner = runner_](Input& received) -> ZxResult<> {
            return AsZxResult(runner->ParseDictionary(std::move(received)));
          })
          .then([callback = std::move(callback)](const ZxResult<>& result) {
            callback(result.is_ok() ? ZX_OK : result.error());
          });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::ReadDictionary(ReadDictionaryCallback callback) {
  callback(AsyncSocketWrite(executor_, runner_->GetDictionaryAsInput()));
}

void ControllerImpl::GetStatus(GetStatusCallback callback) { callback(runner_->CollectStatus()); }

void ControllerImpl::AddMonitor(fidl::InterfaceHandle<Monitor> monitor,
                                AddMonitorCallback callback) {
  FX_DCHECK(runner_);
  runner_->AddMonitor(std::move(monitor));
  callback();
}

void ControllerImpl::GetResults(GetResultsCallback callback) {
  callback(runner_->result(), AsyncSocketWrite(executor_, runner_->result_input()));
}

void ControllerImpl::Execute(FidlInput fidl_input, ExecuteCallback callback) {
  auto task = AsyncSocketRead(executor_, std::move(fidl_input))
                  .and_then([runner = runner_](Input& received) {
                    return runner->Execute(std::move(received));
                  })
                  .then([callback = std::move(callback)](ZxResult<FuzzResult>& result) {
                    callback(std::move(result));
                  });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Minimize(FidlInput fidl_input, MinimizeCallback callback) {
  auto task = AsyncSocketRead(executor_, std::move(fidl_input))
                  .and_then([runner = runner_](Input& received) {
                    return runner->Minimize(std::move(received));
                  })
                  .and_then([executor = executor_](Input& input) {
                    return fpromise::ok(AsyncSocketWrite(executor, std::move(input)));
                  })
                  .then([callback = std::move(callback)](ZxResult<FidlInput>& result) {
                    callback(std::move(result));
                  });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Cleanse(FidlInput fidl_input, CleanseCallback callback) {
  auto task = AsyncSocketRead(executor_, std::move(fidl_input))
                  .and_then([runner = runner_](Input& received) {
                    return runner->Cleanse(std::move(received));
                  })
                  .and_then([executor = executor_](Input& input) {
                    return fpromise::ok(AsyncSocketWrite(executor, std::move(input)));
                  })
                  .then([callback = std::move(callback)](ZxResult<FidlInput>& result) {
                    callback(std::move(result));
                  });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Fuzz(FuzzCallback callback) {
  auto task = runner_->Fuzz()
                  .and_then([executor = executor_](Artifact& artifact) {
                    return fpromise::ok(AsyncSocketWrite(executor, std::move(artifact)));
                  })
                  .then([callback = std::move(callback)](ZxResult<FidlArtifact>& result) {
                    callback(std::move(result));
                  });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Merge(MergeCallback callback) {
  auto task = runner_->Merge().then([callback = std::move(callback)](ZxResult<>& result) {
    callback(result.is_ok() ? ZX_OK : result.error());
  });
  executor_->schedule_task(std::move(task));
}

void ControllerImpl::Stop() {
  if (runner_) {
    executor_->schedule_task(runner_->Stop());
  }
}

}  // namespace fuzzing

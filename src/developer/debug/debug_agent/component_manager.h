// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_

#include <optional>
#include <string>
#include <vector>

#include "src/developer/debug/debug_agent/stdio_handles.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/status.h"

namespace debug_agent {

class DebugAgent;
class Filter;
class ProcessHandle;
class SystemInterface;

// This class manages launching and monitoring Fuchsia components. It is a singleton owned by the
// DebugAgent.
//
// Mostly the debugger deals with processes. It has a limited ability to launch components which
// is handled by this class. Eventually we will need better integration with the Fuchsia component
// framework which would also be managed by this class.
class ComponentManager {
 public:
  virtual ~ComponentManager() = default;

  // Find the component information if the job is the root job of an ELF component.
  virtual std::optional<debug_ipc::ComponentInfo> FindComponentInfo(zx_koid_t job_koid) const = 0;

  // Find the component information if the process runs in the context of a component.
  std::optional<debug_ipc::ComponentInfo> FindComponentInfo(
      const ProcessHandle& process, SystemInterface& system_interface) const;

  // Launches the component with the given command line.
  //
  // DebugAgent is needed here to insert a filter to capture the new component.
  virtual debug::Status LaunchComponent(DebugAgent& debug_agent,
                                        const std::vector<std::string>& argv,
                                        uint64_t* component_id) = 0;

  // Notification that a process has started.
  //
  // If this process launch was a component, this function will fill in the given stdio handles
  // and return the id associated with the component launch.
  //
  // If it was not a component launch, returns false (the caller normally won't know if a launch is
  // a component without asking us, so it isn't necessarily an error).
  virtual uint64_t OnProcessStart(const Filter& filter, StdioHandles& out_stdio) = 0;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_COMPONENT_MANAGER_H_

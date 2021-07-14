// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_H_
#define SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_H_

#include <lib/fpromise/promise.h>

#include <deque>
#include <functional>
#include <limits>
#include <map>
#include <mutex>
#include <string>
#include <vector>

#include "src/developer/debug/shared/logging/file_line_function.h"
#include "src/lib/fxl/macros.h"

#if defined(__Fuchsia__)
#include <zircon/compiler.h>
#else
// The macros for thread annotations aren't set up for non-Fuchsia builds.
#undef __TA_REQUIRES
#define __TA_REQUIRES(arg)
#endif

namespace debug_ipc {

class FDWatcher;
class MessageLoop;

// Context implementation for fpromise::promise integration.
class MessageLoopContext : public fpromise::context {
 public:
  // Pointer must outlive this class.
  explicit MessageLoopContext(MessageLoop* loop) : message_loop_(loop) {}

  // fpromise::context implementation.
  fpromise::executor* executor() const override;
  fpromise::suspended_task suspend_task() override;

 private:
  MessageLoop* message_loop_;
};

// Message loop implementation.
//
// Unlike the one in FXL, this will run on the host in addition to a Zircon target. On Zircon it
// is backed by the async-loop library, on the host (Linux & Mac) it is backed by a poll()
// implementation (see message_loop_poll.h).
//
// This message loop supports several types of tasks:
//  - Bare lambdas.
//  - Delayed lambdas (timers).
//  - fpromise::pending_task objects (normally generated by fpromise::promise).
//  - Async I/O events on file handles.
//
// The target-specific subclass can also watch for Zircon-specific async events (see
// message_loop_target.h for more).
class MessageLoop : public fpromise::executor, public fpromise::suspended_task::resolver {
 public:
  enum class WatchMode {
    kRead,
    kWrite,
    kReadWrite,
  };

  class WatchHandle;

  // There can be only one active MessageLoop in scope per thread at a time.
  //
  // A message loop is active between Init() and Cleanup(). During this period, Current() will
  // return the message loop.
  //
  // Init() / Cleanup() is a separate phase so a message loop can be created and managed on one
  // thread and sent to another thread to actually run (to help with cross-thread task posting).
  MessageLoop();
  ~MessageLoop() override;

  // Init() and Cleanup() must be called on the same thread as Run().
  //
  // Init() returns true on success. On false the error message will be put into the output param.
  virtual bool Init(std::string* error_message);
  virtual void Cleanup();

  // Exits the message loop immediately, not running pending functions. This must be called only on
  // the MessageLoop thread.
  virtual void QuitNow();

  // Returns the current message loop or null if there isn't one.
  static MessageLoop* Current();

  // Runs the message loop.
  void Run();

  // Run until no more tasks are posted. This is not really meant for normal functioning of the
  // debugger. Rather this is geared towards test environments that control what gets inserted into
  // the message loop. This Useful for tests in which tasks post additional tasks.
  //
  // NOTE: OS events (file handles, sockets, signals) are not considered as non-idle tasks.
  //       Basically they're ignored when checking for "idleness".
  void RunUntilNoTasks();

  // Posts the given work to the message loop. It will be added to the end of the work queue.
  void PostTask(FileLineFunction file_line, fit::function<void()> fn);
  void PostTask(FileLineFunction file_line, fpromise::pending_task task);

  // Runs the given task immediately. If it reports a pending completion it will complete
  // asynchronously, otherwise it will complete synchronously. This can be used to start executing
  // a promise without putting it at the back of the message loop.
  //
  // If the task complete asynchronously, it will be added to the queue when it signals a pending
  // completion.
  void RunTask(FileLineFunction file_line, fpromise::pending_task task);

  // Set a task to run after a certain number of milliseconds have elapsed. Granularity is hard to
  // guarantee but the timer shouldn't fire earlier than expected.
  void PostTimer(FileLineFunction file_line, uint64_t delta_ms, fit::function<void()> fn);

  // Starts watching the given file descriptor in the given mode. Returns a WatchHandle that scopes
  // the watch operation (when the handle is destroyed the watcher is unregistered).
  //
  // This function must only be called on the message loop thread.
  //
  // The watcher pointer must outlive the returned WatchHandle. Typically the class implementing the
  // FDWatcher would keep the WatchHandle as a member. Must only be called on the message loop
  // thread.
  //
  // You can only watch a handle once. Note that stdin/stdout/stderr can be the same underlying OS
  // handle, so the caller can only watch one of them.
  virtual WatchHandle WatchFD(WatchMode mode, int fd, FDWatcher* watcher) = 0;

  // fpromise::executor implementation.
  void schedule_task(fpromise::pending_task task) override;

  // fpromise::resolver implementation.
  fpromise::suspended_task::ticket duplicate_ticket(
      fpromise::suspended_task::ticket ticket) override;
  void resolve_ticket(fpromise::suspended_task::ticket ticket, bool resume_task) override;

 protected:
  static constexpr uint64_t kMaxDelay = std::numeric_limits<uint64_t>::max();

  virtual void RunImpl() = 0;

  // Get the value of a monotonic clock in nanoseconds.
  virtual uint64_t GetMonotonicNowNS() const = 0;

  // Used by WatchHandle to unregister a watch. Can be called from any thread without the lock held.
  virtual void StopWatching(int id) = 0;

  // Indicates there are tasks to process. Can be called from any thread and will be called without
  // the lock held.
  virtual void SetHasTasks() = 0;

  // Processes one pending task, returning true if there was work to do, or false if there was
  // nothing. The mutex_ must be held during the call. It will be unlocked during task processing,
  // so the platform implementation that calls it must not assume state did not change across the
  // call.
  bool ProcessPendingTask() __TA_REQUIRES(mutex_);

  // The platform implementation should check should_quit() after every task execution and exit if
  // true.
  bool should_quit() const { return should_quit_; }

  // How much time we should wait before waking up again to process timers.
  uint64_t DelayNS() const;

  // Style guide says this should be private and we should have a protected getter, but that makes
  // the thread annotations much more complicated.
  std::mutex mutex_;

 private:
  friend MessageLoopContext;
  friend WatchHandle;

  // A task is either a bare function or a fpromise::pending function. This is one entry in the
  // task_queue_ of pending runnable tasks.
  struct Task {
    Task() = default;
    Task(FileLineFunction fl, fit::function<void()> fn)
        : file_line(std::move(fl)), task_fn(std::move(fn)) {}
    Task(FileLineFunction fl, fpromise::pending_task pend)
        : file_line(std::move(fl)), pending(std::move(pend)) {}

    FileLineFunction file_line;

    // Only one of these two members will be non-null.
    fit::function<void()> task_fn;
    fpromise::pending_task pending;
  };

  // The data associated with a "ticket". A ticket is the handle behind a fpromise::suspended_task
  // which is used to track fpromise::pending_task objects that have completed asynchronously and to
  // signal that they should be run again.
  struct TicketRecord {
    // A ticket is reference counted, with the references being managed by the
    // fpromise::suspended_task objects. When this reference count gets to 0, the ticket is deleted.
    uint32_t ref_count = 1;

    // Set when the task is resumed. This means it will be moved to the task_queue_ and the task
    // object will be null on this struct. The ticket can exist in this state if there are other
    // fpromise::suspended_task objects that hold a ticket for it, but calling resume() from those
    // will be a no-op.
    bool was_resumed = false;

    // Source of the original post to the message loop.
    FileLineFunction file_line;

    // The actual task. This will be null if the task currently lives on the pending task_queue_.
    // See was_resumed above.
    fpromise::pending_task task;
  };
  using TicketMap = std::map<fpromise::suspended_task::ticket, TicketRecord>;

  // Currently runnable tasks.
  std::deque<Task> task_queue_;

  struct Timer {
    Task task;

    // Expiration time in nanoseconds. The time is absolute and compares to GetMonotonicNowNS.
    uint64_t expiry;
  };
  std::vector<Timer> timers_;

  static bool CompareTimers(const Timer& a, const Timer& b) { return a.expiry >= b.expiry; }

  // Expiration time of the timer which will expire soonest. Returns an upper bound if there are no
  // timers set.
  uint64_t NextExpiryNS() const;

  // Backend for the public PostTask variants above that can handle any task type.
  template <typename TaskType>
  void PostTaskInternal(FileLineFunction file_line, TaskType task);

  // Runs the given task, executing either the lambda or the fpromise::pending_task.
  // The lock must not be held.
  void RunOneTask(Task& task);

  // Backing implementation for the context which gets a suspended_task ticket for the current
  // task.
  fpromise::suspended_task SuspendCurrentTask();

  // Called when a task has reported an async completion. This will save it back to the ticket if
  // one was provided, or it will be deleted if nobody to save it back to the ticket. The lock
  // should not be held.
  void SaveTaskToTicket(fpromise::suspended_task::ticket ticket, FileLineFunction file_line,
                        fpromise::pending_task task);

  bool should_quit_ = false;
  bool should_quit_on_no_more_tasks_ = false;

  MessageLoopContext context_;

  // Tracking information for suspended task tickets. These are handles that are used to suspend or
  // resume tasks.
  TicketMap tickets_;
  fpromise::suspended_task::ticket next_ticket_ = 1;

  // These are only accessed on the thread running this loop since they refer to the "current" task.
  // They do not need locking.
  //
  // The current_task_ticket_ is lazily filled when the current task is suspended. 0 means there is
  // no current task or the current task hasn't been suspended.
  bool current_task_is_promise_ = false;  // For assertions to check proper usage.
  fpromise::suspended_task::ticket current_task_ticket_ = 0;

  FXL_DISALLOW_COPY_AND_ASSIGN(MessageLoop);
};

// Scopes watching a file handle. When the WatchHandle is destroyed, the MessageLoop will stop
// watching the handle. Must only be destroyed on the thread where the MessageLoop is.
//
// Invalid watch handles will have watching() return false.
class MessageLoop::WatchHandle {
 public:
  // Constructs a WatchHandle not watching anything.
  WatchHandle();

  // Constructor used by MessageLoop to make one that watches something.
  WatchHandle(MessageLoop* msg_loop, int id);

  WatchHandle(WatchHandle&&);

  // Stops watching.
  ~WatchHandle();

  WatchHandle& operator=(WatchHandle&& other);

  // Stops watching from the message loop.
  // If the handle is not watching, this doesn't do anything.
  void StopWatching();

  bool watching() const { return id_ > 0; }

 private:
  friend MessageLoop;

  MessageLoop* msg_loop_ = nullptr;
  int id_ = 0;
};

#if !defined(__Fuchsia__)
#undef __TA_REQUIRES
#endif

}  // namespace debug_ipc

#endif  // SRC_DEVELOPER_DEBUG_SHARED_MESSAGE_LOOP_H_

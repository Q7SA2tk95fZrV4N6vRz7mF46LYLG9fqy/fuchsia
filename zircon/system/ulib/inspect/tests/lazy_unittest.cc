// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>

#include <condition_variable>
#include <mutex>
#include <thread>
#include <type_traits>

#include <zxtest/zxtest.h>

using inspect::Inspector;

namespace {

fit::function<fpromise::promise<Inspector>(Inspector&)> OpenChild(std::string next) {
  return [next = std::move(next)](Inspector& insp) { return insp.OpenChild(next); };
}

fit::function<fpromise::result<>(Inspector&)> TakeInspector(fpromise::result<Inspector>* out) {
  return [out](Inspector& result) -> fpromise::result<> {
    *out = fpromise::ok(std::move(result));
    return fpromise::ok();
  };
}

TEST(LazyNode, SimpleLazy) {
  Inspector inspector;
  inspector.GetRoot().CreateLazyNode(
      "test",
      []() {
        auto content = inspect::Inspector();
        content.GetRoot().CreateInt("a", 1234, &content);
        return fpromise::make_ok_promise(content);
      },
      &inspector);
  inspector.GetRoot().CreateLazyNode(
      "next",
      []() {
        auto content = inspect::Inspector();
        content.GetRoot().CreateInt("b", 1234, &content);
        return fpromise::make_ok_promise(content);
      },
      &inspector);

  fpromise::result<Inspector> test0, next1;

  fpromise::single_threaded_executor exec;
  exec.schedule_task(inspector.OpenChild("test-0").and_then(TakeInspector(&test0)));
  exec.schedule_task(inspector.OpenChild("next-1").and_then(TakeInspector(&next1)));
  exec.run();

  auto parsed = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
  ASSERT_EQ(2, parsed.node().links().size());
  const inspect::LinkValue *test_ptr = nullptr, *next_ptr = nullptr;
  for (const auto& l : parsed.node().links()) {
    if (l.name() == "test") {
      test_ptr = &l;
    } else if (l.name() == "next") {
      next_ptr = &l;
    }
  }
  ASSERT_NE(nullptr, test_ptr);
  ASSERT_NE(nullptr, next_ptr);
  EXPECT_EQ("test-0", test_ptr->content());
  EXPECT_EQ(inspect::LinkDisposition::kChild, test_ptr->disposition());
  EXPECT_EQ("next-1", next_ptr->content());
  EXPECT_EQ(inspect::LinkDisposition::kChild, next_ptr->disposition());

  {
    ASSERT_TRUE(test0.is_ok());
    parsed = inspect::ReadFromVmo(test0.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("a", prop.name());
    EXPECT_EQ(1234, val.value());
  }
  {
    ASSERT_TRUE(next1.is_ok());
    parsed = inspect::ReadFromVmo(next1.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("b", prop.name());
    EXPECT_EQ(1234, val.value());
  }
}

TEST(LazyNode, Record) {
  Inspector inspector;
  inspector.GetRoot().RecordLazyNode("node", []() {
    auto content = inspect::Inspector();
    content.GetRoot().RecordInt("a", 1234);
    return fpromise::make_ok_promise(content);
  });
  inspector.GetRoot().RecordLazyValues("vals", []() {
    auto content = inspect::Inspector();
    content.GetRoot().RecordInt("b", 1234);
    return fpromise::make_ok_promise(content);
  });

  fpromise::result<Inspector> node, vals;

  fpromise::single_threaded_executor exec;
  exec.schedule_task(inspector.OpenChild("node-0").and_then(TakeInspector(&node)));
  exec.schedule_task(inspector.OpenChild("vals-1").and_then(TakeInspector(&vals)));
  exec.run();

  auto parsed = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
  ASSERT_EQ(2, parsed.node().links().size());
  const inspect::LinkValue *node_ptr = nullptr, *vals_ptr = nullptr;
  for (const auto& l : parsed.node().links()) {
    if (l.name() == "node") {
      node_ptr = &l;
    } else if (l.name() == "vals") {
      vals_ptr = &l;
    }
  }
  ASSERT_NE(nullptr, node_ptr);
  ASSERT_NE(nullptr, vals_ptr);
  EXPECT_EQ("node-0", node_ptr->content());
  EXPECT_EQ(inspect::LinkDisposition::kChild, node_ptr->disposition());
  EXPECT_EQ("vals-1", vals_ptr->content());
  EXPECT_EQ(inspect::LinkDisposition::kInline, vals_ptr->disposition());

  {
    ASSERT_TRUE(node.is_ok());
    parsed = inspect::ReadFromVmo(node.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("a", prop.name());
    EXPECT_EQ(1234, val.value());
  }
  {
    ASSERT_TRUE(vals.is_ok());
    parsed = inspect::ReadFromVmo(vals.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("b", prop.name());
    EXPECT_EQ(1234, val.value());
  }
}

TEST(LazyNode, LazyRemoval) {
  Inspector inspector;
  {
    auto lazy = inspector.GetRoot().CreateLazyNode("test", []() {
      auto content = inspect::Inspector();
      content.GetRoot().CreateInt("a", 1234, &content);
      return fpromise::make_ok_promise(content);
    });
  }

  auto parsed = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
  EXPECT_EQ(0, parsed.node().links().size());

  EXPECT_EQ(0u, inspector.GetChildNames().size());
}

TEST(LazyNode, NestedLazy) {
  Inspector inspector;
  inspector.GetRoot().CreateLazyNode(
      "test",
      []() {
        Inspector content;
        content.GetRoot().CreateInt("a", 1234, &content);
        content.GetRoot().CreateLazyNode(
            "sub",
            []() {
              Inspector content;
              content.GetRoot().CreateInt("b", 12345, &content);
              return fpromise::make_ok_promise(content);
            },
            &content);
        return fpromise::make_ok_promise(content);
      },
      &inspector);

  fpromise::result<Inspector> test0, sub0;
  fpromise::single_threaded_executor exec;
  exec.schedule_task(inspector.OpenChild("test-0").and_then(TakeInspector(&test0)));
  exec.schedule_task(
      inspector.OpenChild("test-0").and_then(OpenChild("sub-0")).and_then(TakeInspector(&sub0)));
  exec.run();

  {
    auto parsed = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
    ASSERT_EQ(1, parsed.node().links().size());
    EXPECT_EQ("test", parsed.node().links()[0].name());
    EXPECT_EQ("test-0", parsed.node().links()[0].content());
    EXPECT_EQ(inspect::LinkDisposition::kChild, parsed.node().links()[0].disposition());
  }

  {
    ASSERT_TRUE(test0.is_ok());
    auto parsed = inspect::ReadFromVmo(test0.value().DuplicateVmo()).take_value();
    ASSERT_EQ(1, parsed.node().links().size());
    EXPECT_EQ("sub", parsed.node().links()[0].name());
    EXPECT_EQ("sub-0", parsed.node().links()[0].content());
    EXPECT_EQ(inspect::LinkDisposition::kChild, parsed.node().links()[0].disposition());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("a", prop.name());
    EXPECT_EQ(1234, val.value());
  }

  {
    ASSERT_TRUE(sub0.is_ok());
    auto parsed = inspect::ReadFromVmo(sub0.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("b", prop.name());
    EXPECT_EQ(12345, val.value());
  }
}

TEST(LazyNode, AsyncLazy) {
  std::mutex mutex;
  std::condition_variable cv;
  std::unique_ptr<fit::function<void(const Inspector&)>> cb;

  std::thread worker([&] {
    std::unique_lock<std::mutex> lock(mutex);
    cv.wait(lock, [&] { return cb != nullptr; });
    auto content = inspect::Inspector();
    content.GetRoot().CreateInt("a", 1234, &content);
    (*cb)(content);
    cb.reset();
  });

  Inspector inspector;
  inspector.GetRoot().CreateLazyNode(
      "test",
      [&]() {
        fpromise::bridge<Inspector> bridge;
        {
          std::lock_guard<std::mutex> lock(mutex);
          cb = std::make_unique<fit::function<void(const Inspector&)>>(
              [completer = std::move(bridge.completer)](const Inspector& insp) mutable {
                completer.complete_ok(insp);
              });
        }
        cv.notify_one();
        return bridge.consumer.promise_or(fpromise::error());
      },
      &inspector);

  fpromise::result<Inspector> test0;
  fpromise::single_threaded_executor exec;
  exec.schedule_task(inspector.OpenChild("test-0").and_then(TakeInspector(&test0)));
  exec.run();

  worker.join();

  {
    auto parsed = inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
    ASSERT_EQ(1, parsed.node().links().size());
    EXPECT_EQ("test", parsed.node().links()[0].name());
    EXPECT_EQ("test-0", parsed.node().links()[0].content());
    EXPECT_EQ(inspect::LinkDisposition::kChild, parsed.node().links()[0].disposition());
  }

  {
    ASSERT_TRUE(test0.is_ok());
    auto parsed = inspect::ReadFromVmo(test0.value().DuplicateVmo()).take_value();
    ASSERT_EQ(0, parsed.node().links().size());
    ASSERT_EQ(1, parsed.node().properties().size());
    ASSERT_TRUE(parsed.node().properties()[0].Contains<inspect::IntPropertyValue>());
    auto& prop = parsed.node().properties()[0];
    auto& val = prop.Get<inspect::IntPropertyValue>();
    EXPECT_EQ("a", prop.name());
    EXPECT_EQ(1234, val.value());
  }
}

class DeleteThisClass {
 public:
  DeleteThisClass(inspect::Node node) : node_(std::move(node)) {
    ptr_ = std::make_unique<int>(10);
    lazy_ = node_.CreateLazyValues("values", [this] {
      inspect::Inspector insp;
      insp.GetRoot().CreateInt("val", *ptr_, &insp);
      return fpromise::make_ok_promise(std::move(insp));
    });
  }

 private:
  std::unique_ptr<int> ptr_;
  inspect::Node node_;
  inspect::LazyNode lazy_;
};

TEST(LazyNode, LazyLivenessRace) {
  Inspector inspector;

  auto obj = std::make_unique<DeleteThisClass>(inspector.GetRoot().CreateChild("test"));
  auto value_promise = inspector.OpenChild("values-0");
  obj.reset();

  fpromise::result<Inspector> result;
  fpromise::single_threaded_executor exec;
  exec.schedule_task(
      value_promise.then([&](fpromise::result<Inspector>& res) { result = std::move(res); }));
  exec.run();

  EXPECT_TRUE(result.is_ok());
}

}  // namespace

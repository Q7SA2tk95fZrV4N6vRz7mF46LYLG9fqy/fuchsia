// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    component_events::{
        events::{self, Event, EventSource},
        matcher::EventMatcher,
        sequence::{self, EventSequence},
    },
    fidl_fuchsia_sys2 as fsys,
    fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route},
};

#[fuchsia::test]
/// Verifies that when a component has a LogSink in its namespace that the
/// component manager tries to connect to this.
async fn check_logsink_requested() {
    let builder = RealmBuilder::new().await.unwrap();
    let empty_child = builder
        .add_child("empty_child", "#meta/empty.cm", ChildOptions::new().eager())
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&empty_child),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let proxy =
        instance.root.connect_to_protocol_at_exposed_dir::<fsys::EventSourceMarker>().unwrap();

    let mut event_source = EventSource::from_proxy(proxy);

    let expected = EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok()
                    .r#type(events::CapabilityRouted::TYPE)
                    .capability_name("fuchsia.logger.LogSink")
                    .moniker("./empty_child"),
                EventMatcher::ok().r#type(events::Stopped::TYPE).moniker("./empty_child"),
            ],
            sequence::Ordering::Unordered,
        )
        .subscribe_and_expect(&mut event_source)
        .await
        .unwrap();

    instance.start_component_tree().await.unwrap();
    expected.await.unwrap();
}

// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    anyhow::Error,
    bt_manifest_integration_lib::add_fidl_service_handler,
    fidl_fuchsia_bluetooth_bredr::{ProfileMarker, ProfileProxy, ProfileRequestStream},
    fuchsia_async as fasync,
    fuchsia_component::server::ServiceFs,
    fuchsia_component_test::{
        builder::{Capability, CapabilityRoute, ComponentSource, RealmBuilder, RouteEndpoint},
        mock::{Mock, MockHandles},
    },
    futures::{channel::mpsc, SinkExt, StreamExt},
    log::info,
};

/// RFCOMM component URL.
const RFCOMM_URL: &str = "fuchsia-pkg://fuchsia.com/bt-rfcomm-smoke-test#meta/bt-rfcomm.cm";

/// The different events generated by this test.
/// Note: In order to prevent the component under test from terminating, any FIDL channel or
/// request is kept alive.
enum Event {
    /// The fake RFCOMM client connecting to the Profile service and making a search request -
    /// the proxy and the search results stream are kept alive.
    Client(Option<ProfileProxy>),
    /// A BR/EDR Profile service connection request was received - request was
    /// made by the RFCOMM component.
    Profile(Option<ProfileRequestStream>),
}

impl From<ProfileRequestStream> for Event {
    fn from(src: ProfileRequestStream) -> Self {
        Self::Profile(Some(src))
    }
}

/// Represents a fake RFCOMM client that requests the Profile service.
async fn mock_rfcomm_client(
    mut sender: mpsc::Sender<Event>,
    handles: MockHandles,
) -> Result<(), Error> {
    let profile_svc = handles.connect_to_service::<ProfileMarker>()?;
    sender.send(Event::Client(Some(profile_svc))).await.expect("failed sending ack to test");
    Ok(())
}

/// Simulates a component that provides the `bredr.Profile` service.
async fn mock_profile_component(
    sender: mpsc::Sender<Event>,
    handles: MockHandles,
) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    add_fidl_service_handler::<ProfileMarker, _>(&mut fs, sender.clone());
    fs.serve_connection(handles.outgoing_dir.into_channel())?;
    fs.collect::<()>().await;
    Ok(())
}

/// Tests that the v2 RFCOMM component has the correct topology and verifies that
/// it connects and provides the expected services.
#[fasync::run_singlethreaded(test)]
async fn rfcomm_v2_component_topology() {
    fuchsia_syslog::init().unwrap();
    info!("Starting RFCOMM v2 smoke test...");

    let (sender, mut receiver) = mpsc::channel(3);
    let profile_tx = sender.clone();
    let fake_client_tx = sender.clone();

    let mut builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
    // The v2 component under test.
    builder
        .add_component("rfcomm", ComponentSource::url(RFCOMM_URL.to_string()))
        .await
        .expect("Failed adding rfcomm to topology");
    // Mock Profile component to receive bredr.Profile requests.
    builder
        .add_component(
            "fake-profile",
            ComponentSource::Mock(Mock::new({
                move |mock_handles: MockHandles| {
                    let sender = profile_tx.clone();
                    Box::pin(mock_profile_component(sender, mock_handles))
                }
            })),
        )
        .await
        .expect("Failed adding profile mock to topology");
    // Mock RFCOMM client that will connect to the Profile service and make a request.
    builder
        .add_eager_component(
            "fake-rfcomm-client",
            ComponentSource::Mock(Mock::new({
                move |mock_handles: MockHandles| {
                    let sender = fake_client_tx.clone();
                    Box::pin(mock_rfcomm_client(sender, mock_handles))
                }
            })),
        )
        .await
        .expect("Failed adding rfcomm client mock to topology");

    // Set up capabilities.
    builder
        .add_route(CapabilityRoute {
            capability: Capability::protocol("fuchsia.bluetooth.bredr.Profile"),
            source: RouteEndpoint::component("fake-profile"),
            targets: vec![RouteEndpoint::component("rfcomm")],
        })
        .expect("Failed adding route for profile service")
        .add_route(CapabilityRoute {
            capability: Capability::protocol("fuchsia.bluetooth.bredr.Profile"),
            source: RouteEndpoint::component("rfcomm"),
            targets: vec![RouteEndpoint::component("fake-rfcomm-client")],
        })
        .expect("Failed adding route for profile service")
        .add_route(CapabilityRoute {
            capability: Capability::protocol("fuchsia.logger.LogSink"),
            source: RouteEndpoint::AboveRoot,
            targets: vec![
                RouteEndpoint::component("rfcomm"),
                RouteEndpoint::component("fake-profile"),
                RouteEndpoint::component("fake-rfcomm-client"),
            ],
        })
        .expect("Failed adding LogSink route to test components");
    let _test_topology = builder.build().create().await.unwrap();

    // If the routing is correctly configured, we expect two events (in arbitrary order):
    //   1. `bt-rfcomm` connecting to the Profile service provided by `fake-profile`.
    //   2. `fake-rfcomm-client` connecting to the Profile service provided by `bt-rfcomm`.
    let mut events = Vec::new();
    let expected_number_of_events = 2;
    for i in 0..expected_number_of_events {
        let msg = format!("Unexpected error waiting for {:?} event", i);
        events.push(receiver.next().await.expect(&msg));
    }
    assert_eq!(events.len(), expected_number_of_events);
    let _mock_client_event = events
        .iter()
        .find(|&p| std::mem::discriminant(p) == std::mem::discriminant(&Event::Client(None)))
        .expect("Should receive client event");
    let _rfcomm_component_event = events
        .iter()
        .find(|&p| std::mem::discriminant(p) == std::mem::discriminant(&Event::Profile(None)))
        .expect("Should receive component event");

    info!("Finished RFCOMM smoke test");
}

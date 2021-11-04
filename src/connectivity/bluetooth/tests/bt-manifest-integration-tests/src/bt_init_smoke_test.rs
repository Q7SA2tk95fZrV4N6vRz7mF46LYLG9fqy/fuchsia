// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    anyhow::Error,
    fidl::endpoints::DiscoverableProtocolMarker,
    fidl_fuchsia_bluetooth_bredr::{ProfileMarker, ProfileProxy},
    fidl_fuchsia_bluetooth_gatt as fbgatt, fidl_fuchsia_bluetooth_le as fble,
    fidl_fuchsia_bluetooth_rfcomm_test::{RfcommTestMarker, RfcommTestProxy},
    fidl_fuchsia_bluetooth_snoop::{SnoopMarker, SnoopRequestStream},
    fidl_fuchsia_bluetooth_sys::{
        AccessMarker, AccessProxy, BootstrapMarker, BootstrapProxy, ConfigurationMarker,
        ConfigurationProxy, HostWatcherMarker, HostWatcherProxy,
    },
    fidl_fuchsia_device::{NameProviderMarker, NameProviderRequestStream},
    fidl_fuchsia_io2 as fio2,
    fidl_fuchsia_stash::SecureStoreMarker,
    fuchsia_component::server::ServiceFs,
    fuchsia_component_test::{
        mock::MockHandles, ChildProperties, RealmBuilder, RouteBuilder, RouteEndpoint,
    },
    futures::{channel::mpsc, SinkExt, StreamExt},
    realmbuilder_mock_helpers::{add_fidl_service_handler, mock_dev, provide_bt_gap_uses},
    std::sync::Arc,
    tracing::{error, info},
    vfs::{directory::entry::DirectoryEntry, pseudo_directory},
};

const BT_INIT_URL: &str = "fuchsia-pkg://fuchsia.com/bt-init-smoke-test#meta/test-bt-init.cm";

const SECURE_STORE_URL: &str = "fuchsia-pkg://fuchsia.com/bt-init-smoke-test#meta/stash_secure.cm";

// Component monikers.
const BT_INIT_MONIKER: &str = "bt-init";
const MOCK_PROVIDER_MONIKER: &str = "mock-provider";
const MOCK_CLIENT_MONIKER: &str = "mock-client";
const MOCK_DEV_MONIKER: &str = "mock-dev";
const SECURE_STORE_MONIKER: &str = "fake-secure-store";
/// The different events generated by this test.
/// Note: In order to prevent the component under test from terminating, any FIDL request or
/// Proxy is preserved.
enum Event {
    Profile(Option<ProfileProxy>),
    GattServer(Option<fbgatt::Server_Proxy>),
    LeCentral(Option<fble::CentralProxy>),
    LePeripheral(Option<fble::PeripheralProxy>),
    Access(Option<AccessProxy>),
    Bootstrap(Option<BootstrapProxy>),
    Config(Option<ConfigurationProxy>),
    HostWatcher(Option<HostWatcherProxy>),
    RfcommTest(Option<RfcommTestProxy>),
    Snoop(Option<SnoopRequestStream>),
    NameProvider(Option<NameProviderRequestStream>),
    // bt-init will not start up without a working SecureStore, so instead of just notifying the
    // test of requests, we also forward along the requests to a working implementation. As such,
    // there is no need to hold on to the requests in the Event.
    SecureStore,
}

const NUMBER_OF_EVENTS: usize = 12;

impl From<SnoopRequestStream> for Event {
    fn from(src: SnoopRequestStream) -> Self {
        Self::Snoop(Some(src))
    }
}

impl From<NameProviderRequestStream> for Event {
    fn from(src: NameProviderRequestStream) -> Self {
        Self::NameProvider(Some(src))
    }
}

// So that Event can be generically constructed in provide_bt_gap_uses.
impl From<SecureStoreMarker> for Event {
    fn from(_src: SecureStoreMarker) -> Self {
        Self::SecureStore
    }
}

fn dev_bt_host() -> Arc<dyn DirectoryEntry> {
    pseudo_directory! {
        "class" => pseudo_directory! {
            "bt-host" => pseudo_directory! {}
        }
    }
}

/// Represents a fake bt-init client that connects to the services it exposes.
async fn mock_client(mut sender: mpsc::Sender<Event>, handles: MockHandles) -> Result<(), Error> {
    let profile_svc = handles.connect_to_service::<ProfileMarker>()?;
    sender.send(Event::Profile(Some(profile_svc))).await.expect("failed sending ack to test");

    let gatt_server_svc = handles.connect_to_service::<fbgatt::Server_Marker>()?;
    sender
        .send(Event::GattServer(Some(gatt_server_svc)))
        .await
        .expect("failed sending ack to test");

    let le_central_svc = handles.connect_to_service::<fble::CentralMarker>()?;
    sender.send(Event::LeCentral(Some(le_central_svc))).await.expect("failed sending ack to test");

    let le_peripheral_svc = handles.connect_to_service::<fble::PeripheralMarker>()?;
    sender
        .send(Event::LePeripheral(Some(le_peripheral_svc)))
        .await
        .expect("failed sending ack to test");

    let access_svc = handles.connect_to_service::<AccessMarker>()?;
    sender.send(Event::Access(Some(access_svc))).await.expect("failed sending ack to test");

    let bootstrap_svc = handles.connect_to_service::<BootstrapMarker>()?;
    sender.send(Event::Bootstrap(Some(bootstrap_svc))).await.expect("failed sending ack to test");

    let configuration_svc = handles.connect_to_service::<ConfigurationMarker>()?;
    sender.send(Event::Config(Some(configuration_svc))).await.expect("failed sending ack to test");

    let host_watcher_svc = handles.connect_to_service::<HostWatcherMarker>()?;
    sender
        .send(Event::HostWatcher(Some(host_watcher_svc)))
        .await
        .expect("failed sending ack to test");

    let rfcomm_test_svc = handles.connect_to_service::<RfcommTestMarker>()?;
    sender
        .send(Event::RfcommTest(Some(rfcomm_test_svc)))
        .await
        .expect("failed sending ack to test");

    Ok(())
}

/// The component mock that provides the services used by bt-init and its children.
async fn mock_provider(sender: mpsc::Sender<Event>, handles: MockHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    provide_bt_gap_uses(&mut fs, &sender, &handles)?;
    add_fidl_service_handler::<SnoopMarker, _>(&mut fs, sender.clone());

    let _ = fs.serve_connection(handles.outgoing_dir.into_channel())?;
    fs.collect::<()>().await;
    Ok(())
}

// Helper for the common case of routing between bt-init and its mock client.
async fn route_from_bt_init_to_mock_client<S: DiscoverableProtocolMarker>(builder: &RealmBuilder) {
    let _ = builder
        .add_route(
            RouteBuilder::protocol_marker::<S>()
                .source(RouteEndpoint::component(BT_INIT_MONIKER))
                .targets(vec![RouteEndpoint::component(MOCK_CLIENT_MONIKER)]),
        )
        .await
        .expect(&format!("failed routing {} from bt-init to mock client", S::PROTOCOL_NAME));
}

/// Tests that the v2 bt-init component has the correct topology and verifies
/// that it connects and provides the expected services.
#[fuchsia::test]
async fn bt_init_component_topology() {
    info!("Starting bt-init v2 smoke test...");

    let (sender, mut receiver) = mpsc::channel(0);
    let mock_provider_tx = sender.clone();
    let mock_client_tx = sender.clone();

    let builder = RealmBuilder::new().await.expect("Failed to create test realm builder");
    // The v2 component under test.
    let _ = builder
        // Add bt-init to the topology
        .add_child(BT_INIT_MONIKER, BT_INIT_URL.to_string(), ChildProperties::new())
        .await
        .expect("Failed adding bt-init to topology")
        // Implementation of the Secure Store service for use by bt-gap.
        .add_child(SECURE_STORE_MONIKER, SECURE_STORE_URL.to_string(), ChildProperties::new())
        .await
        .expect("Failed adding secure store fake to topology")
        // Provides a mock implementation of the services used by bt-init and its children, and
        // notifies the test of connections to these services.
        .add_mock_child(
            MOCK_PROVIDER_MONIKER,
            move |mock_handles: MockHandles| {
                let sender = mock_provider_tx.clone();
                Box::pin(mock_provider(sender, mock_handles))
            },
            ChildProperties::new(),
        )
        .await
        .expect("Failed adding mock provider to topology")
        .add_mock_child(
            MOCK_DEV_MONIKER,
            move |mock_handles: MockHandles| Box::pin(mock_dev(mock_handles, dev_bt_host())),
            ChildProperties::new(),
        )
        .await
        .expect("Failed adding mock /dev provider to topology")
        // Mock bt-init client that will request all the service exposed by bt-init.
        .add_mock_child(
            MOCK_CLIENT_MONIKER,
            move |mock_handles: MockHandles| {
                let sender = mock_client_tx.clone();
                Box::pin(mock_client(sender, mock_handles))
            },
            ChildProperties::new().eager(),
        )
        .await
        .expect("Failed adding bt-init client mock to topology");

    // Add routes from bt-init to the mock bt-init client.
    route_from_bt_init_to_mock_client::<ProfileMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<fbgatt::Server_Marker>(&builder).await;
    route_from_bt_init_to_mock_client::<fble::CentralMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<fble::PeripheralMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<AccessMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<BootstrapMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<ConfigurationMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<HostWatcherMarker>(&builder).await;
    route_from_bt_init_to_mock_client::<RfcommTestMarker>(&builder).await;

    let _ = builder
        // Add proxy route between secure store and mock provider
        .add_route(
            RouteBuilder::protocol_marker::<SecureStoreMarker>()
                .source(RouteEndpoint::component(SECURE_STORE_MONIKER))
                .targets(vec![RouteEndpoint::component(MOCK_PROVIDER_MONIKER)]),
        )
        .await
        .expect("Failed adding proxy route for Secure Store service")
        // Add routes for bt-init's `use`s from mock-provider.
        .add_route(
            RouteBuilder::protocol_marker::<SecureStoreMarker>()
                .source(RouteEndpoint::component(MOCK_PROVIDER_MONIKER))
                .targets(vec![RouteEndpoint::component(BT_INIT_MONIKER)]),
        )
        .await
        .expect("Failed adding Secure Store route between mock and bt-init")
        .add_route(
            RouteBuilder::protocol_marker::<SnoopMarker>()
                .source(RouteEndpoint::component(MOCK_PROVIDER_MONIKER))
                .targets(vec![RouteEndpoint::component(BT_INIT_MONIKER)]),
        )
        .await
        .expect("Failed adding route for Snoop service")
        .add_route(
            RouteBuilder::protocol_marker::<NameProviderMarker>()
                .source(RouteEndpoint::component(MOCK_PROVIDER_MONIKER))
                .targets(vec![RouteEndpoint::component(BT_INIT_MONIKER)]),
        )
        .await
        .expect("Failed adding route for Name Provider service")
        .add_route(
            RouteBuilder::directory("dev", "/dev", fio2::RW_STAR_DIR)
                .source(RouteEndpoint::component(MOCK_DEV_MONIKER))
                .targets(vec![RouteEndpoint::component(BT_INIT_MONIKER)]),
        )
        .await
        .expect("Failed adding route for bt-host device directory")
        // Proxy LogSink to children
        .add_route(
            RouteBuilder::protocol_marker::<fidl_fuchsia_logger::LogSinkMarker>()
                .source(RouteEndpoint::AboveRoot)
                .targets(vec![
                    RouteEndpoint::component(BT_INIT_MONIKER),
                    RouteEndpoint::component(SECURE_STORE_MONIKER),
                    RouteEndpoint::component(MOCK_PROVIDER_MONIKER),
                    RouteEndpoint::component(MOCK_CLIENT_MONIKER),
                ]),
        )
        .await
        .expect("Failed adding LogSink route to test components")
        // Proxy temp storage to SecureStore impl
        .add_route(
            RouteBuilder::storage("data", "/data")
                .source(RouteEndpoint::AboveRoot)
                .targets(vec![RouteEndpoint::component(SECURE_STORE_MONIKER)]),
        )
        .await
        .expect("Failed adding temp storage route to SecureStore component");
    let mut test_topology = builder.build().await.unwrap();
    let realm_destroyed = test_topology.root.take_destroy_waiter();
    // If the routing is correctly configured, we expect one of each of the Event enum to be
    // sent (so, in total, 10 events)
    let mut events = Vec::new();
    for i in 0..NUMBER_OF_EVENTS {
        let msg = format!("Unexpected error waiting for {:?} event", i);
        let event = receiver.next().await.expect(&msg);
        info!("Got event with discriminant: {:?}", std::mem::discriminant(&event));
        events.push(event);
    }
    assert_eq!(events.len(), NUMBER_OF_EVENTS);
    let discriminants: Vec<_> = events.iter().map(std::mem::discriminant).collect();
    for event in vec![
        Event::Profile(None),
        Event::GattServer(None),
        Event::LeCentral(None),
        Event::LePeripheral(None),
        Event::Access(None),
        Event::Bootstrap(None),
        Event::Config(None),
        Event::HostWatcher(None),
        Event::RfcommTest(None),
        Event::Snoop(None),
        Event::NameProvider(None),
        Event::SecureStore,
    ] {
        let count = discriminants.iter().filter(|&&d| d == std::mem::discriminant(&event)).count();
        assert_eq!(
            count,
            1,
            "Found count of {} for discriminant {:?}, expected 1",
            count,
            std::mem::discriminant(&event)
        );
    }

    // Explicitly drop the topology and wait for realm destruction to ensure that realm components
    // terminate before the local executor stops the mock components. Otherwise, realm components
    // encounter issues with their mocked dependencies disappearing while they are still running.
    // See fxbug.dev/78987 and fxbug.dev/77775 for more background.
    std::mem::drop(test_topology);
    let _ =
        realm_destroyed.await.map_err(|e| error!("failed to wait for realm destruction {:?}", e));
    info!("Finished bt-init smoke test");
}

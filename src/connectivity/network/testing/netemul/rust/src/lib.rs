// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![deny(missing_docs, unreachable_patterns)]

//! Netemul utilities.

use std::convert::{TryFrom as _, TryInto as _};
use std::path::{Path, PathBuf};

use fidl_fuchsia_hardware_ethernet as fethernet;
use fidl_fuchsia_hardware_network as fnetwork;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_dhcp as fnet_dhcp;
use fidl_fuchsia_net_interfaces as fnet_interfaces;
use fidl_fuchsia_net_stack as fnet_stack;
use fidl_fuchsia_net_stack_ext::FidlReturn as _;
use fidl_fuchsia_netemul as fnetemul;
use fidl_fuchsia_netemul_network as fnetemul_network;
use fidl_fuchsia_netstack as fnetstack;
use fidl_fuchsia_posix_socket as fposix_socket;
use fuchsia_zircon as zx;

use anyhow::{anyhow, Context as _};
use fidl::endpoints::Proxy as _;
use futures::{
    future::{FutureExt as _, TryFutureExt as _},
    SinkExt as _, StreamExt as _, TryStreamExt as _,
};

type Result<T = ()> = std::result::Result<T, anyhow::Error>;

/// The default MTU used in in netemul endpoint configurations.
pub const DEFAULT_MTU: u16 = 1500;

/// The port identifier used by netemul for [`NetworkDevice`] endpoints.
pub const PORT_ID: u8 = fnetemul_network::PORT_ID;

/// Abstraction for different endpoint backing types.
pub trait Endpoint: Copy + Clone {
    /// The backing [`EndpointBacking`] for this `Endpoint`.
    ///
    /// [`EndpointBacking`]: fnetemul_network::EndpointBacking
    const NETEMUL_BACKING: fnetemul_network::EndpointBacking;

    /// The relative path from the root device directory where devices of this `Endpoint`
    /// can be discovered.
    const DEV_PATH: &'static str;

    /// Returns the path to an endpoint with the given name in this `Endpoint`'s
    /// device directory.
    fn dev_path(name: &str) -> PathBuf {
        Path::new(Self::DEV_PATH).join(name)
    }

    /// Returns an [`EndpointConfig`] with the provided parameters for this
    /// endpoint type.
    ///
    /// [`EndpointConfig`]: fnetemul_network::EndpointConfig
    fn make_config(mtu: u16, mac: Option<fnet::MacAddress>) -> fnetemul_network::EndpointConfig {
        fnetemul_network::EndpointConfig {
            mtu,
            mac: mac.map(Box::new),
            backing: Self::NETEMUL_BACKING,
        }
    }
}

/// An Ethernet implementation of `Endpoint`.
#[derive(Copy, Clone)]
pub enum Ethernet {}

impl Endpoint for Ethernet {
    const NETEMUL_BACKING: fnetemul_network::EndpointBacking =
        fnetemul_network::EndpointBacking::Ethertap;
    const DEV_PATH: &'static str = "class/ethernet";
}

/// A Network Device implementation of `Endpoint`.
#[derive(Copy, Clone)]
pub enum NetworkDevice {}

impl Endpoint for NetworkDevice {
    const NETEMUL_BACKING: fnetemul_network::EndpointBacking =
        fnetemul_network::EndpointBacking::NetworkDevice;
    const DEV_PATH: &'static str = "class/network";
}

/// A test sandbox backed by a [`fnetemul::SandboxProxy`].
///
/// `TestSandbox` provides various utility methods to set up network realms for
/// use in testing. The lifetime of the `TestSandbox` is tied to the netemul
/// sandbox itself, dropping it will cause all the created realms, networks, and
/// endpoints to be destroyed.
#[must_use]
pub struct TestSandbox {
    sandbox: fnetemul::SandboxProxy,
}

impl TestSandbox {
    /// Creates a new empty sandbox.
    pub fn new() -> Result<TestSandbox> {
        fuchsia_component::client::connect_to_protocol::<fnetemul::SandboxMarker>()
            .context("failed to connect to sandbox protocol")
            .map(|sandbox| TestSandbox { sandbox })
    }

    /// Creates a realm with `name` and `children`.
    // TODO(https://fxbug.dev/84137): Change type of `name` to `Cow<&'static, str>`.
    pub fn create_realm<S, I>(&self, name: S, children: I) -> Result<TestRealm<'_>>
    where
        S: Into<String>,
        I: IntoIterator,
        I::Item: Into<fnetemul::ChildDef>,
    {
        let (realm, server) = fidl::endpoints::create_proxy::<fnetemul::ManagedRealmMarker>()?;
        let name = name.into();
        let () = self.sandbox.create_realm(
            server,
            fnetemul::RealmOptions {
                name: Some(name.clone()),
                children: Some(children.into_iter().map(Into::into).collect()),
                ..fnetemul::RealmOptions::EMPTY
            },
        )?;
        Ok(TestRealm { realm, name, _sandbox: self })
    }

    /// Creates a realm with no components.
    // TODO(https://fxbug.dev/84137): Change type of `name` to `Cow<&'static, str>`.
    pub fn create_empty_realm(&self, name: impl Into<String>) -> Result<TestRealm<'_>> {
        self.create_realm(name, std::iter::empty::<fnetemul::ChildDef>())
    }

    /// Connects to the sandbox's `NetworkContext`.
    fn get_network_context(&self) -> Result<fnetemul_network::NetworkContextProxy> {
        let (ctx, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkContextMarker>()?;
        let () = self.sandbox.get_network_context(server)?;
        Ok(ctx)
    }

    /// Connects to the sandbox's `NetworkManager`.
    fn get_network_manager(&self) -> Result<fnetemul_network::NetworkManagerProxy> {
        let ctx = self.get_network_context()?;
        let (network_manager, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::NetworkManagerMarker>()?;
        let () = ctx.get_network_manager(server)?;
        Ok(network_manager)
    }

    /// Connects to the sandbox's `EndpointManager`.
    fn get_endpoint_manager(&self) -> Result<fnetemul_network::EndpointManagerProxy> {
        let ctx = self.get_network_context()?;
        let (ep_manager, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::EndpointManagerMarker>()?;
        let () = ctx.get_endpoint_manager(server)?;
        Ok(ep_manager)
    }

    /// Creates a new empty network with default configurations and `name`.
    // TODO(https://fxbug.dev/84137): Change type of `name` to `Cow<&'static, str>`.
    pub async fn create_network(&self, name: impl Into<String>) -> Result<TestNetwork<'_>> {
        let name = name.into();
        let netm = self.get_network_manager()?;
        let (status, network) = netm
            .create_network(
                &name,
                fnetemul_network::NetworkConfig {
                    latency: None,
                    packet_loss: None,
                    reorder: None,
                    ..fnetemul_network::NetworkConfig::EMPTY
                },
            )
            .await
            .context("create_network FIDL error")?;
        let () = zx::Status::ok(status).context("create_network failed")?;
        let network = network
            .ok_or_else(|| anyhow::anyhow!("create_network didn't return a valid network"))?
            .into_proxy()?;
        Ok(TestNetwork { network, name, sandbox: self })
    }

    /// Creates a new unattached endpoint with default configurations and `name`.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    // TODO(https://fxbug.dev/84137): Change type of `name` to `Cow<&'static, str>`.
    pub async fn create_endpoint<E, S>(&self, name: S) -> Result<TestEndpoint<'_>>
    where
        S: Into<String>,
        E: Endpoint,
    {
        self.create_endpoint_with(name, E::make_config(DEFAULT_MTU, None)).await
    }

    /// Creates a new unattached endpoint with the provided configuration.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint_with(
        &self,
        // TODO(https://fxbug.dev/84137): Change type to `Cow<&'static, str>`.
        name: impl Into<String>,
        mut config: fnetemul_network::EndpointConfig,
    ) -> Result<TestEndpoint<'_>> {
        let name = {
            let max_len = usize::try_from(fidl_fuchsia_hardware_ethertap::MAX_NAME_LENGTH)
                .context("fuchsia.hardware.ethertap/MAX_NAME_LENGTH does not fit in usize")?;
            let mut name = name.into();
            // NB: Drop characters from the front because it's likely that a name that exceeds the
            // length limit is the full name of a test whose suffix is more informative because
            // nesting of testcases appends suffixes.
            let () = name.replace_range(0..name.len().checked_sub(max_len).unwrap_or(0), "");
            name
        };

        let epm = self.get_endpoint_manager()?;
        let (status, endpoint) =
            epm.create_endpoint(&name, &mut config).await.context("create_endpoint FIDL error")?;
        let () = zx::Status::ok(status).context("create_endpoint failed")?;
        let endpoint = endpoint
            .ok_or_else(|| anyhow::anyhow!("create_endpoint didn't return a valid endpoint"))?
            .into_proxy()?;
        Ok(TestEndpoint { endpoint, name, _sandbox: self })
    }
}

/// Interface configuration used by [`TestRealm::join_network`].
pub enum InterfaceConfig {
    /// Interface is configured with a static address.
    StaticIp(fnet::Subnet),
    /// Interface is configured to use DHCP to obtain an address.
    Dhcp,
    /// No address configuration is performed.
    None,
}

/// A realm within a netemul sandbox.
#[must_use]
pub struct TestRealm<'a> {
    realm: fnetemul::ManagedRealmProxy,
    name: String,
    _sandbox: &'a TestSandbox,
}

impl<'a> TestRealm<'a> {
    /// Connects to a protocol within the realm.
    pub fn connect_to_protocol<S>(&self) -> Result<S::Proxy>
    where
        S: fidl::endpoints::DiscoverableProtocolMarker,
    {
        (|| {
            let (proxy, server_end) =
                fidl::endpoints::create_proxy::<S>().context("create proxy")?;
            let () = self
                .realm
                .connect_to_protocol(S::PROTOCOL_NAME, None, server_end.into_channel())
                .context("connect to protocol")?;
            Result::Ok(proxy)
        })()
        .context(S::PROTOCOL_NAME)
    }

    /// Gets the relative moniker of the root of the managed realm.
    pub async fn get_moniker(&self) -> Result<String> {
        self.realm.get_moniker().await.context("failed to call get moniker")
    }

    /// Stops the specified child component of the managed realm.
    pub async fn stop_child_component(&self, child_name: &str) -> Result {
        self.realm
            .stop_child_component(child_name)
            .await?
            .map_err(zx::Status::from_raw)
            .with_context(|| format!("failed to stop child component '{}'", child_name))
    }

    /// Like [`join_network_with`], but uses default endpoint configurations.
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the maximum length.
    pub async fn join_network<E, S>(
        &self,
        network: &TestNetwork<'a>,
        // TODO(https://fxbug.dev/84137): Change type to `Cow<&'static, str>`.
        ep_name: S,
        if_config: &InterfaceConfig,
    ) -> Result<TestInterface<'a>>
    where
        E: Endpoint,
        S: Into<String>,
    {
        let endpoint =
            network.create_endpoint::<E, _>(ep_name).await.context("failed to create endpoint")?;
        self.install_endpoint(endpoint, if_config, None).await
    }

    /// Joins `network` with by creating an endpoint with `ep_config` and
    /// installing it into the realm with `if_config`.
    ///
    /// `join_network_with` is a helper to create a new endpoint `ep_name`
    /// attached to `network` and configure it with `if_config`. Returns a
    /// [`TestInterface`] which is already added to this realm's netstack, link
    /// online, enabled, and configured according to `config`.
    ///
    /// Note that this realm needs a Netstack for this operation to succeed.
    ///
    /// Characters may be dropped from the front of `ep_name` if it exceeds the maximum length.
    pub async fn join_network_with(
        &self,
        network: &TestNetwork<'a>,
        // TODO(https://fxbug.dev/84137): Change type to `Cow<&'static, str>`.
        ep_name: impl Into<String>,
        ep_config: fnetemul_network::EndpointConfig,
        if_config: &InterfaceConfig,
    ) -> Result<TestInterface<'a>> {
        let endpoint = network
            .create_endpoint_with(ep_name, ep_config)
            .await
            .context("failed to create endpoint")?;
        self.install_endpoint(endpoint, if_config, None).await
    }

    /// Installs and configures the endpoint in this realm.
    ///
    /// Note that if `name` is not `None`, the string must fit within interface name limits.
    pub async fn install_endpoint(
        &self,
        endpoint: TestEndpoint<'a>,
        config: &InterfaceConfig,
        name: Option<String>,
    ) -> Result<TestInterface<'a>> {
        let interface = endpoint
            .into_interface_in_realm_with_name(self, name)
            .await
            .context("failed to add endpoint")?;
        let () = interface.set_link_up(true).await.context("failed to start endpoint")?;
        let () = match config {
            InterfaceConfig::StaticIp(addr) => {
                interface.add_ip_addr(*addr).await.context("failed to add static IP")?
            }
            InterfaceConfig::Dhcp => {
                interface.start_dhcp().await.context("failed to start DHCP")?;
            }
            InterfaceConfig::None => (),
        };
        let () = interface.enable_interface().await.context("failed to enable interface")?;

        // Wait for Netstack to observe interface up so callers can safely
        // assume the state of the world on return.
        let interface_state = self
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("failed to connect to fuchsia.net.interfaces/State")?;
        let () = fidl_fuchsia_net_interfaces_ext::wait_interface_with_id(
            fidl_fuchsia_net_interfaces_ext::event_stream_from_state(&interface_state)?,
            &mut fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(interface.id()),
            |&fidl_fuchsia_net_interfaces_ext::Properties { online, .. }| {
                // TODO(https://github.com/rust-lang/rust/issues/80967): use bool::then_some.
                online.then(|| ())
            },
        )
        .await
        .context("failed to observe interface up")?;

        Ok(interface)
    }

    /// Adds a device to the realm's virtual device filesystem.
    pub async fn add_virtual_device(&self, e: &TestEndpoint<'_>, path: &Path) -> Result {
        let path = path
            .to_str()
            .with_context(|| format!("convert {} to str", path.display()))?
            .to_string();
        let (device, device_server_end) =
            fidl::endpoints::create_endpoints::<fnetemul_network::DeviceProxy_Marker>()
                .context("create endpoints")?;
        e.get_proxy_(device_server_end).context("get proxy")?;

        self.realm
            .add_device(&path, device)
            .await
            .context("add device")?
            .map_err(zx::Status::from_raw)
            .context("add device error")
    }

    /// Removes a device from the realm's virtual device filesystem.
    pub async fn remove_virtual_device(&self, path: &Path) -> Result {
        let path = path.to_str().with_context(|| format!("convert {} to str", path.display()))?;
        self.realm
            .remove_device(path)
            .await
            .context("remove device")?
            .map_err(zx::Status::from_raw)
            .context("remove device error")
    }

    /// Creates a Datagram [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm.
    pub async fn datagram_socket(
        &self,
        domain: fidl_fuchsia_posix_socket::Domain,
        proto: fidl_fuchsia_posix_socket::DatagramSocketProtocol,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;
        let sock = socket_provider
            .datagram_socket(domain, proto)
            .await
            .context("failed to call socket")?
            .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))
            .context("failed to create socket")?;

        Ok(fdio::create_fd(sock.into()).context("failed to create fd")?)
    }

    /// Creates a Stream [`socket2::Socket`] backed by the implementation of
    /// `fuchsia.posix.socket/Provider` in this realm.
    pub async fn stream_socket(
        &self,
        domain: fidl_fuchsia_posix_socket::Domain,
        proto: fidl_fuchsia_posix_socket::StreamSocketProtocol,
    ) -> Result<socket2::Socket> {
        let socket_provider = self
            .connect_to_protocol::<fposix_socket::ProviderMarker>()
            .context("failed to connect to socket provider")?;
        let sock = socket_provider
            .stream_socket(domain, proto)
            .await
            .context("failed to call socket")?
            .map_err(|e| std::io::Error::from_raw_os_error(e.into_primitive()))
            .context("failed to create socket")?;

        Ok(fdio::create_fd(sock.into()).context("failed to create fd")?)
    }

    /// Shuts down the realm.
    pub async fn shutdown(&self) -> Result {
        let () = self.realm.shutdown().context("call shutdown")?;
        let events = self
            .realm
            .take_event_stream()
            .try_collect::<Vec<_>>()
            .await
            .context("error on realm event stream")?;
        // Ensure there are no more events sent on the event stream after `OnShutdown`.
        matches::assert_matches!(events[..], [fnetemul::ManagedRealmEvent::OnShutdown {}]);
        Ok(())
    }

    /// Constructs an ICMP socket.
    pub async fn icmp_socket<Ip: ping::IpExt>(&self) -> Result<fuchsia_async::net::DatagramSocket> {
        let sock = self
            .datagram_socket(
                Ip::DOMAIN_FIDL,
                fidl_fuchsia_posix_socket::DatagramSocketProtocol::IcmpEcho,
            )
            .await
            .context("failed to create ICMP datagram socket")?;
        fuchsia_async::net::DatagramSocket::new_from_socket(sock)
            .context("failed to create async ICMP datagram socket")
    }

    /// Sends a single ICMP echo request to `addr`, and waits for the echo reply.
    pub async fn ping_once<Ip: ping::IpExt>(&self, addr: Ip::Addr, seq: u16) -> Result {
        let icmp_sock = self.icmp_socket::<Ip>().await?;

        const MESSAGE: &'static str = "hello, world";
        let (mut sink, mut stream) = ping::new_unicast_sink_and_stream::<
            Ip,
            _,
            { MESSAGE.len() + ping::ICMP_HEADER_LEN },
        >(&icmp_sock, &addr, MESSAGE.as_bytes());

        let send_fut = sink.send(seq).map_err(anyhow::Error::new);
        let recv_fut = stream.try_next().map(|r| match r {
            Ok(Some(got)) if got == seq => Ok(()),
            Ok(Some(got)) => Err(anyhow!("unexpected echo reply; got: {}, want: {}", got, seq)),
            Ok(None) => Err(anyhow!("echo reply stream ended unexpectedly")),
            Err(e) => Err(anyhow::Error::from(e)),
        });

        let ((), ()) = futures::future::try_join(send_fut, recv_fut)
            .await
            .with_context(|| format!("failed to ping from {} to {}", self.name, addr,))?;
        Ok(())
    }

    // TODO(https://fxbug.dev/88245): Remove this function when pinging only
    // once is free from NUD-related issues and is guaranteed to succeed.
    /// Sends ICMP echo requests to `addr` on a 1-second interval until a response
    /// is received.
    pub async fn ping<Ip: ping::IpExt>(&self, addr: Ip::Addr) -> Result {
        let icmp_sock = self.icmp_socket::<Ip>().await?;

        const MESSAGE: &'static str = "hello, world";
        let (mut sink, stream) = ping::new_unicast_sink_and_stream::<
            Ip,
            _,
            { MESSAGE.len() + ping::ICMP_HEADER_LEN },
        >(&icmp_sock, &addr, MESSAGE.as_bytes());

        let mut seq = 0;
        let mut interval_stream =
            fuchsia_async::Interval::new(fuchsia_async::Duration::from_seconds(1));
        let mut stream = stream.fuse();
        loop {
            futures::select! {
                opt = interval_stream.next() => {
                    let () = opt.ok_or_else(|| anyhow!("ping interval stream ended unexpectedly"))?;
                    seq += 1;
                    let () = sink.send(seq).map_err(anyhow::Error::new).await?;
                }
                r = stream.try_next() => {
                    return match r {
                        Ok(Some(got)) if got <= seq => Ok(()),
                        Ok(Some(got)) => {
                            Err(anyhow!("unexpected echo reply; got: {}, want: {}", got, seq))
                        }
                        Ok(None) => Err(anyhow!("echo reply stream ended unexpectedly")),
                        Err(e) => Err(anyhow::Error::from(e)),
                    };
                }
            }
        }
    }
}

/// A virtual Network.
///
/// `TestNetwork` is a single virtual broadcast domain backed by Netemul.
/// Created through [`TestSandbox::create_network`].
#[must_use]
pub struct TestNetwork<'a> {
    network: fnetemul_network::NetworkProxy,
    name: String,
    sandbox: &'a TestSandbox,
}

impl<'a> TestNetwork<'a> {
    /// Attaches `ep` to this network.
    pub async fn attach_endpoint(&self, ep: &TestEndpoint<'a>) -> Result<()> {
        let status =
            self.network.attach_endpoint(&ep.name).await.context("attach_endpoint FIDL error")?;
        let () = zx::Status::ok(status).context("attach_endpoint failed")?;
        Ok(())
    }

    /// Creates a new endpoint with `name` attached to this network.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    // TODO(https://fxbug.dev/84137): Change type to `Cow<&'static, str>`.
    pub async fn create_endpoint<E, S>(&self, name: S) -> Result<TestEndpoint<'a>>
    where
        E: Endpoint,
        S: Into<String>,
    {
        let ep = self
            .sandbox
            .create_endpoint::<E, _>(name)
            .await
            .with_context(|| format!("failed to create endpoint for network {}", self.name))?;
        let () = self.attach_endpoint(&ep).await.with_context(|| {
            format!("failed to attach endpoint {} to network {}", ep.name, self.name)
        })?;
        Ok(ep)
    }

    /// Creates a new endpoint with `name` and `config` attached to this network.
    ///
    /// Characters may be dropped from the front of `name` if it exceeds the maximum length.
    pub async fn create_endpoint_with(
        &self,
        // TODO(https://fxbug.dev/84137): Change type to `Cow<&'static, str>`.
        name: impl Into<String>,
        config: fnetemul_network::EndpointConfig,
    ) -> Result<TestEndpoint<'a>> {
        let ep = self
            .sandbox
            .create_endpoint_with(name, config)
            .await
            .with_context(|| format!("failed to create endpoint for network {}", self.name))?;
        let () = self.attach_endpoint(&ep).await.with_context(|| {
            format!("failed to attach endpoint {} to network {}", ep.name, self.name)
        })?;
        Ok(ep)
    }

    /// Returns a fake endpoint.
    pub fn create_fake_endpoint(&self) -> Result<TestFakeEndpoint<'a>> {
        let (endpoint, server) =
            fidl::endpoints::create_proxy::<fnetemul_network::FakeEndpointMarker>()
                .context("failed to create launcher proxy")?;
        let () = self.network.create_fake_endpoint(server)?;
        return Ok(TestFakeEndpoint { endpoint, _sandbox: self.sandbox });
    }
}

/// A virtual network endpoint backed by Netemul.
#[must_use]
pub struct TestEndpoint<'a> {
    endpoint: fnetemul_network::EndpointProxy,
    name: String,
    _sandbox: &'a TestSandbox,
}

impl<'a> std::ops::Deref for TestEndpoint<'a> {
    type Target = fnetemul_network::EndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

/// A virtual fake network endpoint backed by Netemul.
#[must_use]
pub struct TestFakeEndpoint<'a> {
    endpoint: fnetemul_network::FakeEndpointProxy,
    _sandbox: &'a TestSandbox,
}

impl<'a> std::ops::Deref for TestFakeEndpoint<'a> {
    type Target = fnetemul_network::FakeEndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

impl<'a> TestFakeEndpoint<'a> {
    /// Return a stream of frames.
    ///
    /// Frames will be yielded as they are read from the fake endpoint.
    pub fn frame_stream(
        &self,
    ) -> impl futures::Stream<Item = std::result::Result<(Vec<u8>, u64), fidl::Error>> + '_ {
        futures::stream::try_unfold(&self.endpoint, |ep| ep.read().map_ok(move |r| Some((r, ep))))
    }
}

impl<'a> TestEndpoint<'a> {
    /// Gets access to this device's virtual Ethernet device.
    ///
    /// Note that an error is returned if the Endpoint is not a
    /// [`fnetemul_network::DeviceConnection::Ethernet`].
    pub async fn get_ethernet(
        &self,
    ) -> Result<fidl::endpoints::ClientEnd<fethernet::DeviceMarker>> {
        match self
            .get_device()
            .await
            .with_context(|| format!("failed to get device connection for {}", self.name))?
        {
            fnetemul_network::DeviceConnection::Ethernet(e) => Ok(e),
            fnetemul_network::DeviceConnection::NetworkDevice(_) => {
                Err(anyhow::anyhow!("Endpoint {} is not an Ethernet device", self.name))
            }
        }
    }

    /// Gets access to this device's virtual Network device.
    ///
    /// Note that an error is returned if the Endpoint is not a
    /// [`fnetemul_network::DeviceConnection::NetworkDevice`].
    pub async fn get_netdevice(
        &self,
    ) -> Result<(
        fidl::endpoints::ClientEnd<fnetwork::DeviceMarker>,
        fidl::endpoints::ClientEnd<fnetwork::MacAddressingMarker>,
    )> {
        match self
            .get_device()
            .await
            .with_context(|| format!("failed to get device connection for {}", self.name))?
        {
            fnetemul_network::DeviceConnection::NetworkDevice(n) => {
                Self::connect_netdevice_protocols(n)
            }
            fnetemul_network::DeviceConnection::Ethernet(_) => {
                Err(anyhow::anyhow!("Endpoint {} is not a Network Device", self.name))
            }
        }
    }

    /// Helper function to retrieve the protocols from a netdevice client end.
    fn connect_netdevice_protocols(
        netdevice: fidl::endpoints::ClientEnd<fnetwork::DeviceInstanceMarker>,
    ) -> Result<(
        fidl::endpoints::ClientEnd<fnetwork::DeviceMarker>,
        fidl::endpoints::ClientEnd<fnetwork::MacAddressingMarker>,
    )> {
        // TODO(http://fxbug.dev/85061): Do not automatically connect to port
        // once Netstack exposes FIDL that is port-aware and we've migrated to
        // fuchsia.net.interfaces.admin to add devices to the stack.
        let netdevice: fnetwork::DeviceInstanceProxy = netdevice.into_proxy()?;
        let (device, device_server_end) =
            fidl::endpoints::create_proxy::<fnetwork::DeviceMarker>()?;
        let (port, port_server_end) = fidl::endpoints::create_proxy::<fnetwork::PortMarker>()?;
        let () = device.get_port(PORT_ID, port_server_end)?;
        let (mac, mac_server_end) =
            fidl::endpoints::create_endpoints::<fnetwork::MacAddressingMarker>()?;
        let () = netdevice.get_device(device_server_end)?;
        let () = port.get_mac(mac_server_end)?;
        // No other references exist, we just created this proxy, unwrap is safe.
        let device =
            fidl::endpoints::ClientEnd::new(device.into_channel().unwrap().into_zx_channel());
        Ok((device, mac))
    }

    /// Adds this endpoint to `stack`, returning the interface identifier.
    pub async fn add_to_stack(&self, realm: &TestRealm<'a>) -> Result<u64> {
        let stack = realm
            .connect_to_protocol::<fnet_stack::StackMarker>()
            .context("failed to connect to stack")?;
        Ok(match self.get_device().await.context("get_device failed")? {
            fnetemul_network::DeviceConnection::Ethernet(eth) => {
                stack.add_ethernet_interface(&self.name, eth).await.squash_result()?
            }
            fnetemul_network::DeviceConnection::NetworkDevice(netdevice) => {
                let (device, mac) = Self::connect_netdevice_protocols(netdevice)?;
                stack
                    .add_interface(
                        fnet_stack::InterfaceConfig::EMPTY,
                        &mut fnet_stack::DeviceDefinition::Ethernet(
                            fnet_stack::EthernetDeviceDefinition {
                                network_device: device,
                                mac: mac,
                            },
                        ),
                    )
                    .await
                    .squash_result()?
            }
        })
    }

    async fn add_to_stack_with_name(&self, realm: &TestRealm<'a>, name: String) -> Result<u64> {
        let stack = realm
            .connect_to_protocol::<fnet_stack::StackMarker>()
            .context("failed to connect to stack")?;
        let netstack = realm
            .connect_to_protocol::<fnetstack::NetstackMarker>()
            .context("failed to connect to netstack")?;
        match self.get_device().await.context("get_device failed")? {
            fnetemul_network::DeviceConnection::Ethernet(eth) => netstack
                .add_ethernet_device(
                    &self.name,
                    &mut fnetstack::InterfaceConfig { name, filepath: String::new(), metric: 0 },
                    eth,
                )
                .await
                .context("add_ethernet_device FIDL error")?
                .map_err(fuchsia_zircon::Status::from_raw)
                .map(u64::from)
                .context("add_ethernet_device failed"),
            fnetemul_network::DeviceConnection::NetworkDevice(netdevice) => {
                let (device, mac) = Self::connect_netdevice_protocols(netdevice)?;
                stack
                    .add_interface(
                        fnet_stack::InterfaceConfig {
                            name: Some(name),
                            topopath: None,
                            metric: None,
                            ..fnet_stack::InterfaceConfig::EMPTY
                        },
                        &mut fnet_stack::DeviceDefinition::Ethernet(
                            fnet_stack::EthernetDeviceDefinition {
                                network_device: device,
                                mac: mac,
                            },
                        ),
                    )
                    .await
                    .squash_result()
            }
        }
    }

    /// Consumes this `TestEndpoint` and tries to add it to the Netstack in
    /// `realm`, returning a [`TestInterface`] on success.
    pub async fn into_interface_in_realm(self, realm: &TestRealm<'a>) -> Result<TestInterface<'a>> {
        self.into_interface_in_realm_with_name(realm, None).await
    }

    async fn into_interface_in_realm_with_name(
        self,
        realm: &TestRealm<'a>,
        name: Option<String>,
    ) -> Result<TestInterface<'a>> {
        let id = match name {
            Some(name) => self.add_to_stack_with_name(realm, name).await,
            None => self.add_to_stack(realm).await,
        }
        .with_context(|| format!("failed to add {} to realm {}", self.name, realm.name))?;
        let stack = realm
            .connect_to_protocol::<fnet_stack::StackMarker>()
            .context("failed to connect to stack")?;
        let netstack = realm
            .connect_to_protocol::<fnetstack::NetstackMarker>()
            .context("failed to connect to netstack")?;
        let interface_state = realm
            .connect_to_protocol::<fnet_interfaces::StateMarker>()
            .context("failed to connect to interfaces state")?;
        Ok(TestInterface { endpoint: self, id, stack, netstack, interface_state })
    }
}

/// A [`TestEndpoint`] that is installed in a realm's Netstack.
#[must_use]
pub struct TestInterface<'a> {
    endpoint: TestEndpoint<'a>,
    id: u64,
    stack: fnet_stack::StackProxy,
    netstack: fnetstack::NetstackProxy,
    interface_state: fnet_interfaces::StateProxy,
}

impl<'a> std::ops::Deref for TestInterface<'a> {
    type Target = fnetemul_network::EndpointProxy;

    fn deref(&self) -> &Self::Target {
        &self.endpoint
    }
}

impl<'a> TestInterface<'a> {
    /// Gets the interface identifier.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Enable interface.
    ///
    /// Equivalent to `stack.enable_interface(test_interface.id())`.
    pub async fn enable_interface(&self) -> Result<()> {
        self.stack.enable_interface(self.id).await.squash_result().with_context(|| {
            format!("stack.enable_interface for endpoint {} failed", self.endpoint.name)
        })
    }

    /// Disable interface.
    ///
    /// Equivalent to `stack.disable_interface(test_interface.id())`.
    pub async fn disable_interface(&self) -> Result<()> {
        self.stack.disable_interface(self.id).await.squash_result().with_context(|| {
            format!("stack.disable_interface for endpoint {} failed", self.endpoint.name)
        })
    }

    /// Add interface address.
    ///
    /// Equivalent to `stack.add_interface_address(test_interface.id(), &mut addr)`.
    pub async fn add_ip_addr(&self, mut addr: fnet::Subnet) -> Result<()> {
        self.stack.add_interface_address(self.id, &mut addr).await.squash_result().with_context(
            || {
                format!(
                    "stack.add_interface_address({}, {:?}) for endpoint {} failed",
                    self.id, addr, self.endpoint.name
                )
            },
        )
    }

    /// Gets the interface's properties.
    async fn get_properties(&self) -> Result<fidl_fuchsia_net_interfaces_ext::Properties> {
        let properties = fidl_fuchsia_net_interfaces_ext::existing(
            fidl_fuchsia_net_interfaces_ext::event_stream_from_state(&self.interface_state)?,
            fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(self.id),
        )
        .await
        .context("failed to get existing interfaces")?;
        match properties {
            fidl_fuchsia_net_interfaces_ext::InterfaceState::Unknown(id) => Err(anyhow::anyhow!(
                "could not find interface {} for endpoint {}",
                id,
                self.endpoint.name
            )),
            fidl_fuchsia_net_interfaces_ext::InterfaceState::Known(properties) => Ok(properties),
        }
    }

    /// Gets the interface's addresses.
    pub async fn get_addrs(&self) -> Result<Vec<fidl_fuchsia_net_interfaces_ext::Address>> {
        let fidl_fuchsia_net_interfaces_ext::Properties {
            addresses,
            id: _,
            name: _,
            device_class: _,
            online: _,
            has_default_ipv4_route: _,
            has_default_ipv6_route: _,
        } = self.get_properties().await?;
        Ok(addresses)
    }

    /// Gets a fuchsia.net.interfaces/Watcher proxy.
    pub fn get_interfaces_watcher(&self) -> Result<fidl_fuchsia_net_interfaces::WatcherProxy> {
        let (watcher, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_net_interfaces::WatcherMarker>()
                .context("failed to create fuchsia.net.interfaces/Watcher proxy")?;
        let () = self
            .interface_state
            .get_watcher(fidl_fuchsia_net_interfaces::WatcherOptions::EMPTY, server_end)
            .context("failed to create interface property watcher")?;
        Ok(watcher)
    }

    async fn get_dhcp_client(&self) -> Result<fnet_dhcp::ClientProxy> {
        let (dhcp_client, server_end) = fidl::endpoints::create_proxy::<fnet_dhcp::ClientMarker>()
            .context("failed to create endpoints for fuchsia.net.dhcp.Client")?;

        let () = self
            .netstack
            .get_dhcp_client(
                self.id.try_into().with_context(|| {
                    format!("interface ID should fit in a u32; ID = {}", self.id)
                })?,
                server_end,
            )
            .await
            .context("failed to call netstack.get_dhcp_client")?
            .map_err(zx::Status::from_raw)
            .context("failed to get dhcp client")?;
        Ok(dhcp_client)
    }

    /// Starts DHCP on this interface.
    pub async fn start_dhcp(&self) -> Result<()> {
        let dhcp_client = self.get_dhcp_client().await?;
        let () = dhcp_client
            .start()
            .await
            .context("failed to call dhcp_client.start")?
            .map_err(zx::Status::from_raw)
            .context("failed to start dhcp client")?;

        Ok(())
    }

    /// Stops DHCP on this interface.
    pub async fn stop_dhcp(&self) -> Result<()> {
        let dhcp_client = self.get_dhcp_client().await?;
        let () = dhcp_client
            .stop()
            .await
            .context("failed to call dhcp_client.stop")?
            .map_err(zx::Status::from_raw)
            .context("failed to stop dhcp client")?;

        Ok(())
    }
}

/// Get the [`socket2::Domain`] for `addr`.
fn get_socket2_domain(addr: &std::net::SocketAddr) -> fidl_fuchsia_posix_socket::Domain {
    let domain = match addr {
        std::net::SocketAddr::V4(_) => fidl_fuchsia_posix_socket::Domain::Ipv4,
        std::net::SocketAddr::V6(_) => fidl_fuchsia_posix_socket::Domain::Ipv6,
    };

    domain
}

/// Trait describing UDP sockets that can be bound in a testing realm.
pub trait RealmUdpSocket: Sized {
    /// Creates a UDP socket in `realm` bound to `addr`.
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;
}

impl RealmUdpSocket for std::net::UdpSocket {
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .datagram_socket(
                    get_socket2_domain(&addr),
                    fidl_fuchsia_posix_socket::DatagramSocketProtocol::Udp,
                )
                .await
                .context("failed to create socket")?;

            let () = sock.bind(&addr.into()).context("bind failed")?;

            Result::Ok(sock.into())
        }
        .boxed_local()
    }
}

impl RealmUdpSocket for fuchsia_async::net::UdpSocket {
    fn bind_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        std::net::UdpSocket::bind_in_realm(realm, addr)
            .and_then(|udp| {
                futures::future::ready(
                    fuchsia_async::net::UdpSocket::from_socket(udp)
                        .context("failed to create fuchsia_async socket"),
                )
            })
            .boxed_local()
    }
}

/// Trait describing TCP listeners bound in a testing realm.
pub trait RealmTcpListener: Sized {
    /// Creates a TCP listener in `realm` bound to `addr`.
    fn listen_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        Self::listen_in_realm_with(realm, addr, |_: &socket2::Socket| Ok(()))
    }

    /// Creates a TCP listener by creating a Socket2 socket in `realm`. Closure `setup` is called
    /// with the reference of the socket before the socket is bound to `addr`.
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;
}

impl RealmTcpListener for std::net::TcpListener {
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .stream_socket(
                    get_socket2_domain(&addr),
                    fidl_fuchsia_posix_socket::StreamSocketProtocol::Tcp,
                )
                .await
                .context("failed to create server socket")?;
            let () = setup(&sock)?;
            let () = sock.bind(&addr.into()).context("failed to bind server socket")?;
            // Use 128 for the listen() backlog, same as the original implementation of TcpListener
            // in Rust std (see https://doc.rust-lang.org/src/std/sys_common/net.rs.html#386).
            let () = sock.listen(128).context("failed to listen on server socket")?;

            Result::Ok(sock.into())
        }
        .boxed_local()
    }
}

impl RealmTcpListener for fuchsia_async::net::TcpListener {
    fn listen_in_realm_with<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
        setup: impl FnOnce(&socket2::Socket) -> Result<()> + 'a,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        std::net::TcpListener::listen_in_realm_with(realm, addr, setup)
            .and_then(|listener| {
                futures::future::ready(
                    fuchsia_async::net::TcpListener::from_std(listener)
                        .context("failed to create fuchsia_async socket"),
                )
            })
            .boxed_local()
    }
}

/// Trait describing TCP streams in a testing realm.
pub trait RealmTcpStream: Sized {
    /// Creates a TCP stream in `realm` connected to `addr`.
    fn connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>>;

    // TODO: Implement this trait for std::net::TcpStream.
}

impl RealmTcpStream for fuchsia_async::net::TcpStream {
    fn connect_in_realm<'a>(
        realm: &'a TestRealm<'a>,
        addr: std::net::SocketAddr,
    ) -> futures::future::LocalBoxFuture<'a, Result<Self>> {
        async move {
            let sock = realm
                .stream_socket(
                    get_socket2_domain(&addr),
                    fidl_fuchsia_posix_socket::StreamSocketProtocol::Tcp,
                )
                .await
                .context("failed to create socket")?;

            let stream = fuchsia_async::net::TcpStream::connect_from_raw(sock, addr)
                .context("failed to create client tcp stream")?
                .await
                .context("failed to connect to server")?;

            Result::Ok(stream)
        }
        .boxed_local()
    }
}

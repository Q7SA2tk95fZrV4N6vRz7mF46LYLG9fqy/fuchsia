// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A networking stack.

#![cfg_attr(not(fuzz), no_std)]
// In case we roll the toolchain and something we're using as a feature has been
// stabilized.
#![allow(stable_features)]
// Tracking: https://github.com/rust-lang/rust/issues/31844.
#![feature(min_specialization)]
#![deny(missing_docs, unreachable_patterns)]
// Turn off checks for dead code, but only when building for fuzzing or
// benchmarking. This allows fuzzers and benchmarks to be written as part of
// the crate, with access to test utilities, without a bunch of build errors
// due to unused code. These checks are turned back on in the 'fuzz' and
// 'benchmark' modules.
#![cfg_attr(any(fuzz, benchmark), allow(dead_code, unused_imports, unused_macros))]

// TODO(https://github.com/rust-lang-nursery/portability-wg/issues/11): remove
// this module.
extern crate fakealloc as alloc;

// TODO(https://github.com/dtolnay/thiserror/pull/64): remove this module.
#[cfg(not(fuzz))]
extern crate fakestd as std;

#[macro_use]
mod macros;

mod algorithm;
#[cfg(test)]
pub mod benchmarks;
pub mod context;
mod data_structures;
mod device;
pub mod error;
#[cfg(fuzz)]
mod fuzz;
mod ip;
mod socket;
#[cfg(test)]
mod testutil;
mod transport;

use log::trace;

pub use crate::{
    data_structures::{Entry, IdMap, IdMapCollection, IdMapCollectionKey},
    device::{
        add_ethernet_device, add_loopback_device, get_ipv4_configuration, get_ipv6_configuration,
        receive_frame, remove_device, update_ipv4_configuration, update_ipv6_configuration,
        DeviceId, DeviceLayerEventDispatcher,
    },
    error::{LocalAddressError, NetstackError, RemoteAddressError, SocketError},
    ip::{
        device::{
            dad::DadEvent,
            route_discovery::Ipv6RouteDiscoveryEvent,
            slaac::SlaacConfiguration,
            state::{IpDeviceConfiguration, Ipv4DeviceConfiguration, Ipv6DeviceConfiguration},
            IpAddressState, IpDeviceEvent,
        },
        forwarding::AddRouteError,
        icmp,
        socket::{IpSockCreationError, IpSockRouteError, IpSockSendError, IpSockUnroutableError},
        AddableEntry, AddableEntryEither, EntryEither, IpDeviceIdContext, IpExt, IpLayerEvent,
        Ipv4StateBuilder, Ipv6StateBuilder, TransportIpContext,
    },
    transport::{
        udp::{
            connect_udp, create_udp_unbound, get_udp_bound_device, get_udp_conn_info,
            get_udp_listener_info, get_udp_posix_reuse_port, listen_udp, remove_udp_conn,
            remove_udp_listener, remove_udp_unbound, send_udp, send_udp_conn, send_udp_listener,
            set_bound_udp_device, set_udp_posix_reuse_port, set_unbound_udp_device,
            BufferUdpContext, BufferUdpStateContext, UdpBoundId, UdpConnId, UdpConnInfo,
            UdpContext, UdpListenerId, UdpListenerInfo, UdpSendError, UdpSendListenerError,
            UdpSockCreationError, UdpSocketId, UdpStateContext, UdpStateNonSyncContext,
            UdpUnboundId,
        },
        TransportStateBuilder,
    },
};

use alloc::vec::Vec;
use core::{fmt::Debug, marker::PhantomData, time};

use net_types::{
    ip::{AddrSubnetEither, IpAddr, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr, SubnetEither},
    SpecifiedAddr,
};
use packet::{Buf, BufferMut, EmptyBuf};

use crate::{
    context::{EventContext, RngContext, TimerContext},
    device::{DeviceLayerState, DeviceLayerTimerId},
    ip::{
        device::{Ipv4DeviceTimerId, Ipv6DeviceTimerId},
        icmp::{BufferIcmpContext, IcmpContext},
        IpLayerTimerId, Ipv4State, Ipv6State,
    },
    transport::{TransportLayerState, TransportLayerTimerId},
};

/// Map an expression over either version of one or more addresses.
///
/// `map_addr_version!` when given a value of a type which is an enum with two
/// variants - `V4` and `V6` - matches on the variants, and for both variants,
/// invokes an expression on the inner contents. `$addr` is both the name of the
/// variable to match on, and the name that the address will be bound to for the
/// scope of the expression.
///
/// `map_addr_version!` when given a list of values and their types (all enums
/// with variants `V4` and `V6`), matches on the tuple of values and invokes the
/// `$match` expression when all values are of the same variant. Otherwise the
/// `$mismatch` expression is invoked.
///
/// To make it concrete, the expression `map_addr_version!(bar: Foo; blah(bar))`
/// desugars to:
///
/// ```rust,ignore
/// match bar {
///     Foo::V4(bar) => blah(bar),
///     Foo::V6(bar) => blah(bar),
/// }
/// ```
///
/// Also,
/// `map_addr_version!((foo: Foo, bar: Bar); blah(foo, bar), unreachable!())`
/// desugars to:
///
/// ```rust,ignore
/// match (foo, bar) {
///     (Foo::V4(foo), Bar::V4(bar)) => blah(foo, bar),
///     (Foo::V6(foo), Bar::V6(bar)) => blah(foo, bar),
///     _ => unreachable!(),
/// }
/// ```
#[macro_export]
macro_rules! map_addr_version {
    ($addr:ident: $ty:tt; $expr:expr) => {
        match $addr {
            $ty::V4($addr) => $expr,
            $ty::V6($addr) => $expr,
        }
    };
    ($addr:ident: $ty:tt; $expr_v4:expr, $expr_v6:expr) => {
        match $addr {
            $ty::V4($addr) => $expr_v4,
            $ty::V6($addr) => $expr_v6,
        }
    };
    (( $( $addr:ident : $ty:tt ),+ ); $match:expr, $mismatch:expr) => {
        match ( $( $addr ),+ ) {
            ( $( $ty::V4( $addr ) ),+ ) => $match,
            ( $( $ty::V6( $addr ) ),+ ) => $match,
            _ => $mismatch,
        }
    };
    (( $( $addr:ident : $ty:tt ),+ ); $match_v4:expr, $match_v6:expr, $mismatch:expr) => {
        match ( $( $addr ),+ ) {
            ( $( $ty::V4( $addr ) ),+ ) => $match_v4,
            ( $( $ty::V6( $addr ) ),+ ) => $match_v6,
            _ => $mismatch,
        }
    };
    (( $( $addr:ident : $ty:tt ),+ ); $match:expr, $mismatch:expr,) => {
        map_addr_version!(($( $addr: $ty ),+); $match, $mismatch)
    };
}

/// A builder for [`StackState`].
#[derive(Default, Clone)]
pub struct StackStateBuilder {
    transport: TransportStateBuilder,
    ipv4: Ipv4StateBuilder,
    ipv6: Ipv6StateBuilder,
}

impl StackStateBuilder {
    /// Get the builder for the transport layer state.
    pub fn transport_builder(&mut self) -> &mut TransportStateBuilder {
        &mut self.transport
    }

    /// Get the builder for the IPv4 state.
    pub fn ipv4_builder(&mut self) -> &mut Ipv4StateBuilder {
        &mut self.ipv4
    }

    /// Get the builder for the IPv6 state.
    pub fn ipv6_builder(&mut self) -> &mut Ipv6StateBuilder {
        &mut self.ipv6
    }

    /// Consume this builder and produce a `StackState`.
    pub fn build<I: Instant>(self) -> StackState<I> {
        StackState {
            transport: self.transport.build(),
            ipv4: self.ipv4.build(),
            ipv6: self.ipv6.build(),
            device: Default::default(),
            #[cfg(test)]
            test_counters: Default::default(),
        }
    }
}

/// The state associated with the network stack.
pub struct StackState<I: Instant> {
    transport: TransportLayerState,
    ipv4: Ipv4State<I, DeviceId>,
    ipv6: Ipv6State<I, DeviceId>,
    device: DeviceLayerState<I>,
    #[cfg(test)]
    test_counters: core::cell::RefCell<testutil::TestCounters>,
}

impl<I: Instant> Default for StackState<I> {
    fn default() -> StackState<I> {
        StackStateBuilder::default().build()
    }
}

/// The non synchronized context for the stack with a buffer.
pub trait BufferNonSyncContextInner<B: BufferMut>: DeviceLayerEventDispatcher<B> {}
impl<B: BufferMut, C: DeviceLayerEventDispatcher<B>> BufferNonSyncContextInner<B> for C {}

/// The non synchronized context for the stack with a buffer.
pub trait BufferNonSyncContext<B: BufferMut>:
    NonSyncContext + BufferNonSyncContextInner<B>
{
}
impl<B: BufferMut, C: NonSyncContext + BufferNonSyncContextInner<B>> BufferNonSyncContext<B> for C {}

/// The non-synchronized context for the stack.
pub trait NonSyncContext:
    BufferNonSyncContextInner<Buf<Vec<u8>>>
    + BufferNonSyncContextInner<EmptyBuf>
    + RngContext
    + TimerContext<TimerId>
    + EventContext<IpDeviceEvent<DeviceId, Ipv4>>
    + EventContext<IpDeviceEvent<DeviceId, Ipv6>>
    + EventContext<IpLayerEvent<DeviceId, Ipv4>>
    + EventContext<IpLayerEvent<DeviceId, Ipv6>>
    + EventContext<DadEvent<DeviceId>>
    + EventContext<Ipv6RouteDiscoveryEvent<DeviceId>>
{
}
impl<
        C: BufferNonSyncContextInner<Buf<Vec<u8>>>
            + BufferNonSyncContextInner<EmptyBuf>
            + RngContext
            + TimerContext<TimerId>
            + EventContext<IpDeviceEvent<DeviceId, Ipv4>>
            + EventContext<IpDeviceEvent<DeviceId, Ipv6>>
            + EventContext<IpLayerEvent<DeviceId, Ipv4>>
            + EventContext<IpLayerEvent<DeviceId, Ipv6>>
            + EventContext<DadEvent<DeviceId>>
            + EventContext<Ipv6RouteDiscoveryEvent<DeviceId>>,
    > NonSyncContext for C
{
}

/// The synchronized context.
pub struct SyncCtx<D: EventDispatcher, NonSyncCtx: NonSyncContext> {
    /// Contains the state of the stack.
    pub state: StackState<NonSyncCtx::Instant>,
    /// The dispatcher, take a look at [`EventDispatcher`] for more details.
    pub dispatcher: D,
    /// A marker for the non-synchronized context type.
    pub non_sync_ctx_marker: PhantomData<NonSyncCtx>,
}

/// Context available during the execution of the netstack.
///
/// `Ctx` provides access to the state of the netstack and to an event
/// dispatcher which can be used to emit events and schedule timers. A mutable
/// reference to a `Ctx` is passed to every function in the netstack.
pub struct Ctx<D: EventDispatcher, NonSyncCtx: NonSyncContext> {
    /// The synchronized context.
    pub sync_ctx: SyncCtx<D, NonSyncCtx>,
    /// The non-synchronized context.
    pub non_sync_ctx: NonSyncCtx,
}

impl<D: EventDispatcher + Default, NonSyncCtx: NonSyncContext + Default> Default
    for Ctx<D, NonSyncCtx>
where
    StackState<NonSyncCtx::Instant>: Default,
{
    fn default() -> Ctx<D, NonSyncCtx> {
        Ctx {
            sync_ctx: SyncCtx {
                state: StackState::default(),
                dispatcher: D::default(),
                non_sync_ctx_marker: PhantomData,
            },
            non_sync_ctx: Default::default(),
        }
    }
}

impl<D: EventDispatcher, NonSyncCtx: NonSyncContext + Default> Ctx<D, NonSyncCtx> {
    /// Constructs a new `Ctx`.
    pub fn new(state: StackState<NonSyncCtx::Instant>, dispatcher: D) -> Ctx<D, NonSyncCtx> {
        Ctx {
            sync_ctx: SyncCtx { state, dispatcher, non_sync_ctx_marker: PhantomData },
            non_sync_ctx: Default::default(),
        }
    }

    /// Constructs a new `Ctx` using the default `StackState`.
    pub fn with_default_state(dispatcher: D) -> Ctx<D, NonSyncCtx> {
        Ctx {
            sync_ctx: SyncCtx {
                state: StackState::default(),
                dispatcher,
                non_sync_ctx_marker: PhantomData,
            },
            non_sync_ctx: Default::default(),
        }
    }
}

impl<D: EventDispatcher + Default, NonSyncCtx: NonSyncContext + Default> Ctx<D, NonSyncCtx> {
    /// Construct a new `Ctx` using the default dispatcher.
    pub fn with_default_dispatcher(state: StackState<NonSyncCtx::Instant>) -> Ctx<D, NonSyncCtx> {
        Ctx {
            sync_ctx: SyncCtx { state, dispatcher: D::default(), non_sync_ctx_marker: PhantomData },
            non_sync_ctx: Default::default(),
        }
    }
}

/// The identifier for any timer event.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct TimerId(TimerIdInner);

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
enum TimerIdInner {
    /// A timer event in the device layer.
    DeviceLayer(DeviceLayerTimerId),
    /// A timer event in the transport layer.
    _TransportLayer(TransportLayerTimerId),
    /// A timer event in the IP layer.
    IpLayer(IpLayerTimerId),
    /// A timer event for an IPv4 device.
    Ipv4Device(Ipv4DeviceTimerId<DeviceId>),
    /// A timer event for an IPv6 device.
    Ipv6Device(Ipv6DeviceTimerId<DeviceId>),
    /// A no-op timer event (used for tests)
    #[cfg(test)]
    Nop(usize),
}

impl From<DeviceLayerTimerId> for TimerId {
    fn from(id: DeviceLayerTimerId) -> TimerId {
        TimerId(TimerIdInner::DeviceLayer(id))
    }
}

impl From<Ipv4DeviceTimerId<DeviceId>> for TimerId {
    fn from(id: Ipv4DeviceTimerId<DeviceId>) -> TimerId {
        TimerId(TimerIdInner::Ipv4Device(id))
    }
}

impl From<Ipv6DeviceTimerId<DeviceId>> for TimerId {
    fn from(id: Ipv6DeviceTimerId<DeviceId>) -> TimerId {
        TimerId(TimerIdInner::Ipv6Device(id))
    }
}

impl From<IpLayerTimerId> for TimerId {
    fn from(id: IpLayerTimerId) -> TimerId {
        TimerId(TimerIdInner::IpLayer(id))
    }
}

impl_timer_context!(TimerId, DeviceLayerTimerId, TimerId(TimerIdInner::DeviceLayer(id)), id);
impl_timer_context!(TimerId, IpLayerTimerId, TimerId(TimerIdInner::IpLayer(id)), id);
impl_timer_context!(
    TimerId,
    Ipv4DeviceTimerId<DeviceId>,
    TimerId(TimerIdInner::Ipv4Device(id)),
    id
);
impl_timer_context!(
    TimerId,
    Ipv6DeviceTimerId<DeviceId>,
    TimerId(TimerIdInner::Ipv6Device(id)),
    id
);

/// Handles a generic timer event.
pub fn handle_timer<D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    sync_ctx: &mut SyncCtx<D, NonSyncCtx>,
    ctx: &mut NonSyncCtx,
    id: TimerId,
) {
    trace!("handle_timer: dispatching timerid: {:?}", id);

    match id {
        TimerId(TimerIdInner::DeviceLayer(x)) => {
            device::handle_timer(sync_ctx, ctx, x);
        }
        TimerId(TimerIdInner::_TransportLayer(x)) => {
            transport::handle_timer(sync_ctx, x);
        }
        TimerId(TimerIdInner::IpLayer(x)) => {
            ip::handle_timer(sync_ctx, ctx, x);
        }
        TimerId(TimerIdInner::Ipv4Device(x)) => {
            ip::device::handle_ipv4_timer(sync_ctx, ctx, x);
        }
        TimerId(TimerIdInner::Ipv6Device(x)) => {
            ip::device::handle_ipv6_timer(sync_ctx, ctx, x);
        }
        #[cfg(test)]
        TimerId(TimerIdInner::Nop(_)) => {
            increment_counter!(sync_ctx, "timer::nop");
        }
    }
}

/// A type representing an instant in time.
///
/// `Instant` can be implemented by any type which represents an instant in
/// time. This can include any sort of real-world clock time (e.g.,
/// [`std::time::Instant`]) or fake time such as in testing.
pub trait Instant: Sized + Ord + Copy + Clone + Debug + Send + Sync {
    /// Returns the amount of time elapsed from another instant to this one.
    ///
    /// # Panics
    ///
    /// This function will panic if `earlier` is later than `self`.
    fn duration_since(&self, earlier: Self) -> time::Duration;

    /// Returns `Some(t)` where `t` is the time `self + duration` if `t` can be
    /// represented as `Instant` (which means it's inside the bounds of the
    /// underlying data structure), `None` otherwise.
    fn checked_add(&self, duration: time::Duration) -> Option<Self>;

    /// Returns `Some(t)` where `t` is the time `self - duration` if `t` can be
    /// represented as `Instant` (which means it's inside the bounds of the
    /// underlying data structure), `None` otherwise.
    fn checked_sub(&self, duration: time::Duration) -> Option<Self>;
}

/// An `EventDispatcher` which supports sending buffers of a given type.
///
/// `D: BufferDispatcher<B>` is shorthand for `D: EventDispatcher +
/// DeviceLayerEventDispatcher<B>`.
pub trait BufferDispatcher<B: BufferMut>:
    EventDispatcher
    + BufferIcmpContext<Ipv4, B>
    + BufferIcmpContext<Ipv6, B>
    + BufferUdpContext<Ipv4, B>
    + BufferUdpContext<Ipv6, B>
{
}
impl<
        B: BufferMut,
        D: EventDispatcher
            + BufferIcmpContext<Ipv4, B>
            + BufferIcmpContext<Ipv6, B>
            + BufferUdpContext<Ipv4, B>
            + BufferUdpContext<Ipv6, B>,
    > BufferDispatcher<B> for D
{
}

// TODO(joshlf): Should we add a `for<'a> DeviceLayerEventDispatcher<&'a mut
// [u8]>` bound? Would anything get more efficient if we were able to stack
// allocate internally-generated buffers?

/// An object which can dispatch events to a real system.
///
/// An `EventDispatcher` provides access to a real system. It provides the
/// ability to emit events and schedule timers. Each layer of the stack
/// provides its own event dispatcher trait which specifies the types of actions
/// that must be supported in order to support that layer of the stack. The
/// `EventDispatcher` trait is a sub-trait of all of these traits.
pub trait EventDispatcher:
    IcmpContext<Ipv4>
    + IcmpContext<Ipv6>
    + BufferIcmpContext<Ipv4, Buf<Vec<u8>>>
    + BufferIcmpContext<Ipv4, EmptyBuf>
    + BufferIcmpContext<Ipv6, Buf<Vec<u8>>>
    + BufferIcmpContext<Ipv6, EmptyBuf>
    + UdpContext<Ipv4>
    + UdpContext<Ipv6>
    + BufferUdpContext<Ipv4, Buf<Vec<u8>>>
    + BufferUdpContext<Ipv4, EmptyBuf>
    + BufferUdpContext<Ipv6, Buf<Vec<u8>>>
    + BufferUdpContext<Ipv6, EmptyBuf>
{
}

impl<
        D: IcmpContext<Ipv4>
            + IcmpContext<Ipv6>
            + BufferIcmpContext<Ipv4, Buf<Vec<u8>>>
            + BufferIcmpContext<Ipv4, EmptyBuf>
            + BufferIcmpContext<Ipv6, Buf<Vec<u8>>>
            + BufferIcmpContext<Ipv6, EmptyBuf>
            + UdpContext<Ipv4>
            + UdpContext<Ipv6>
            + BufferUdpContext<Ipv4, Buf<Vec<u8>>>
            + BufferUdpContext<Ipv4, EmptyBuf>
            + BufferUdpContext<Ipv6, Buf<Vec<u8>>>
            + BufferUdpContext<Ipv6, EmptyBuf>,
    > EventDispatcher for D
{
}

/// Get all IPv4 and IPv6 address/subnet pairs configured on a device
pub fn get_all_ip_addr_subnets<'a, D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    ctx: &'a SyncCtx<D, NonSyncCtx>,
    device: DeviceId,
) -> impl 'a + Iterator<Item = AddrSubnetEither> {
    let addr_v4 = crate::ip::device::get_assigned_ipv4_addr_subnets(ctx, device)
        .map(|a| AddrSubnetEither::V4(a));
    let addr_v6 = crate::ip::device::get_assigned_ipv6_addr_subnets(ctx, device)
        .map(|a| AddrSubnetEither::V6(a));

    addr_v4.chain(addr_v6)
}

/// Set the IP address and subnet for a device.
pub fn add_ip_addr_subnet<D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    sync_ctx: &mut SyncCtx<D, NonSyncCtx>,
    ctx: &mut NonSyncCtx,
    device: DeviceId,
    addr_sub: AddrSubnetEither,
) -> error::Result<()> {
    map_addr_version!(
        addr_sub: AddrSubnetEither;
        crate::device::add_ip_addr_subnet(sync_ctx, ctx, device, addr_sub)
    )
    .map_err(From::from)
}

/// Delete an IP address on a device.
pub fn del_ip_addr<D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    sync_ctx: &mut SyncCtx<D, NonSyncCtx>,
    ctx: &mut NonSyncCtx,
    device: DeviceId,
    addr: IpAddr<SpecifiedAddr<Ipv4Addr>, SpecifiedAddr<Ipv6Addr>>,
) -> error::Result<()> {
    map_addr_version!(
        addr: IpAddr;
        crate::device::del_ip_addr(sync_ctx, ctx, device, &addr)
    )
    .map_err(From::from)
}

/// Adds a route to the forwarding table.
pub fn add_route<D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    sync_ctx: &mut SyncCtx<D, NonSyncCtx>,
    ctx: &mut NonSyncCtx,
    entry: AddableEntryEither<DeviceId>,
) -> Result<(), AddRouteError> {
    let (subnet, device, gateway) = entry.into_subnet_device_gateway();
    match (device, gateway) {
        (Some(device), None) => map_addr_version!(
            subnet: SubnetEither;
            crate::ip::add_device_route::<Ipv4, _, _>(sync_ctx, ctx, subnet, device),
            crate::ip::add_device_route::<Ipv6, _, _>(sync_ctx, ctx, subnet, device)
        )
        .map_err(From::from),
        (None, Some(next_hop)) => {
            let next_hop = next_hop.into();
            map_addr_version!(
                (subnet: SubnetEither, next_hop: IpAddr);
                crate::ip::add_route::<Ipv4, _, _>(sync_ctx, ctx, subnet, next_hop),
                crate::ip::add_route::<Ipv6, _, _>(sync_ctx, ctx, subnet, next_hop),
                unreachable!()
            )
        }
        x => todo!("TODO(https://fxbug.dev/96680): support setting gateway route with device; (device, gateway) = {:?}", x),
    }
}

/// Delete a route from the forwarding table, returning `Err` if no
/// route was found to be deleted.
pub fn del_route<D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    sync_ctx: &mut SyncCtx<D, NonSyncCtx>,
    ctx: &mut NonSyncCtx,
    subnet: SubnetEither,
) -> error::Result<()> {
    map_addr_version!(
        subnet: SubnetEither;
        crate::ip::del_route::<Ipv4, _, _>(sync_ctx, ctx, subnet),
        crate::ip::del_route::<Ipv6, _, _>(sync_ctx, ctx, subnet)
    )
    .map_err(From::from)
}

/// Get all the routes.
pub fn get_all_routes<'a, D: EventDispatcher, NonSyncCtx: NonSyncContext>(
    ctx: &'a SyncCtx<D, NonSyncCtx>,
) -> impl 'a + Iterator<Item = EntryEither<DeviceId>> {
    let v4_routes = ip::iter_all_routes::<_, _, Ipv4Addr>(ctx);
    let v6_routes = ip::iter_all_routes::<_, _, Ipv6Addr>(ctx);
    v4_routes.cloned().map(From::from).chain(v6_routes.cloned().map(From::from))
}

#[cfg(test)]
mod tests {
    use net_types::{
        ip::{Ip, Ipv4, Ipv6},
        Witness,
    };

    use super::*;
    use crate::testutil::{DummyEventDispatcherBuilder, TestIpExt};

    fn test_add_remove_ip_addresses<I: Ip + TestIpExt>() {
        let config = I::DUMMY_CONFIG;
        let Ctx { mut sync_ctx, mut non_sync_ctx } = DummyEventDispatcherBuilder::default().build();
        let device = crate::add_ethernet_device(
            &mut sync_ctx,
            &mut non_sync_ctx,
            config.local_mac,
            Ipv6::MINIMUM_LINK_MTU.into(),
        );
        crate::device::testutil::enable_device(&mut sync_ctx, &mut non_sync_ctx, device);

        let ip: IpAddr = I::get_other_ip_address(1).get().into();
        let prefix = config.subnet.prefix();
        let addr_subnet = AddrSubnetEither::new(ip, prefix).unwrap();

        // IP doesn't exist initially.
        assert_eq!(get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet), None);

        // Add IP (OK).
        let () = add_ip_addr_subnet(&mut sync_ctx, &mut non_sync_ctx, device, addr_subnet).unwrap();
        assert_eq!(
            get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet),
            Some(addr_subnet)
        );

        // Add IP again (already exists).
        assert_eq!(
            add_ip_addr_subnet(&mut sync_ctx, &mut non_sync_ctx, device, addr_subnet).unwrap_err(),
            NetstackError::Exists
        );
        assert_eq!(
            get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet),
            Some(addr_subnet)
        );

        // Add IP with different subnet (already exists).
        let wrong_addr_subnet = AddrSubnetEither::new(ip, prefix - 1).unwrap();
        assert_eq!(
            add_ip_addr_subnet(&mut sync_ctx, &mut non_sync_ctx, device, wrong_addr_subnet)
                .unwrap_err(),
            NetstackError::Exists
        );
        assert_eq!(
            get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet),
            Some(addr_subnet)
        );

        let ip = SpecifiedAddr::new(ip).unwrap();
        // Del IP (ok).
        let () = del_ip_addr(&mut sync_ctx, &mut non_sync_ctx, device, ip.into()).unwrap();
        assert_eq!(get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet), None);

        // Del IP again (not found).
        assert_eq!(
            del_ip_addr(&mut sync_ctx, &mut non_sync_ctx, device, ip.into()).unwrap_err(),
            NetstackError::NotFound
        );
        assert_eq!(get_all_ip_addr_subnets(&sync_ctx, device).find(|&a| a == addr_subnet), None);
    }

    #[test]
    fn test_add_remove_ipv4_addresses() {
        test_add_remove_ip_addresses::<Ipv4>();
    }

    #[test]
    fn test_add_remove_ipv6_addresses() {
        test_add_remove_ip_addresses::<Ipv6>();
    }
}

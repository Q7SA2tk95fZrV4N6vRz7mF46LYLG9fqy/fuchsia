// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    crate::{
        capability::{CapabilitySource, InternalCapability},
        model::{
            error::ModelError,
            events::{
                dispatcher::{EventDispatcher, ScopeMetadata},
                error::EventsError,
                event::SyncMode,
                filter::EventFilter,
                stream::EventStream,
                synthesizer::{EventSynthesisProvider, EventSynthesizer},
            },
            hooks::{Event as ComponentEvent, EventType, HasEventType, Hook, HooksRegistration},
            model::Model,
            moniker::AbsoluteMoniker,
            realm::Realm,
            routing,
        },
    },
    async_trait::async_trait,
    cm_rust::{CapabilityName, UseDecl, UseEventDecl},
    fuchsia_trace as trace,
    futures::lock::Mutex,
    std::{
        collections::HashMap,
        sync::{Arc, Weak},
    },
};

#[derive(Debug)]
pub struct RoutedEvent {
    pub source_name: CapabilityName,
    pub scopes: Vec<ScopeMetadata>,
}

#[derive(Debug)]
pub struct RouteEventsResult {
    /// Maps from source name to a set of scope monikers
    mapping: HashMap<CapabilityName, Vec<ScopeMetadata>>,
}

impl RouteEventsResult {
    fn new() -> Self {
        Self { mapping: HashMap::new() }
    }

    fn insert(&mut self, source_name: CapabilityName, scope: ScopeMetadata) {
        let values = self.mapping.entry(source_name).or_insert(Vec::new());
        if !values.contains(&scope) {
            values.push(scope);
        }
    }

    pub fn len(&self) -> usize {
        self.mapping.len()
    }

    pub fn contains_event(&self, event_name: &CapabilityName) -> bool {
        self.mapping.contains_key(event_name)
    }

    pub fn to_vec(self) -> Vec<RoutedEvent> {
        self.mapping
            .into_iter()
            .map(|(source_name, scopes)| RoutedEvent { source_name, scopes })
            .collect()
    }
}

#[derive(Clone)]
pub struct SubscriptionOptions {
    /// Determines how event routing is done.
    pub subscription_type: SubscriptionType,
    /// Determines whether component manager waits for a response from the
    /// event receiver.
    pub sync_mode: SyncMode,
}

impl SubscriptionOptions {
    pub fn new(subscription_type: SubscriptionType, sync_mode: SyncMode) -> Self {
        Self { subscription_type, sync_mode }
    }
}

impl Default for SubscriptionOptions {
    fn default() -> SubscriptionOptions {
        SubscriptionOptions {
            subscription_type: SubscriptionType::Normal(AbsoluteMoniker::root()),
            sync_mode: SyncMode::Async,
        }
    }
}

#[derive(Clone)]
pub enum SubscriptionType {
    /// Indicates that event routing will be bypassed and all events can be subscribed.
    Debug,
    /// Indicates that this is a production-worthy event subscription and the target is
    /// the provided AbsoluteMoniker.
    Normal(AbsoluteMoniker),
}

/// Subscribes to events from multiple tasks and sends events to all of them.
pub struct EventRegistry {
    model: Weak<Model>,
    dispatcher_map: Arc<Mutex<HashMap<CapabilityName, Vec<Weak<EventDispatcher>>>>>,
    event_synthesizer: EventSynthesizer,
}

impl EventRegistry {
    pub fn new(model: Weak<Model>) -> Self {
        let event_synthesizer = EventSynthesizer::new(model.clone());
        Self { model, dispatcher_map: Arc::new(Mutex::new(HashMap::new())), event_synthesizer }
    }

    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![
            // This hook must be registered with all events.
            // However, a task will only receive events to which it subscribed.
            HooksRegistration::new(
                "EventRegistry",
                EventType::values(),
                Arc::downgrade(self) as Weak<dyn Hook>,
            ),
        ]
    }
    /// Register a provider for an synthesized event.
    pub fn register_synthesis_provider(
        &mut self,
        event: EventType,
        provider: Arc<dyn EventSynthesisProvider>,
    ) {
        self.event_synthesizer.register_provider(event, provider);
    }

    /// Subscribes to events of a provided set of EventTypes.
    pub async fn subscribe(
        &self,
        options: &SubscriptionOptions,
        events: Vec<CapabilityName>,
    ) -> Result<EventStream, ModelError> {
        // Register event capabilities if any. It identifies the sources of these events (might be the
        // containing realm or this realm itself). It consturcts an "allow-list tree" of events and
        // realms.
        let events = match &options.subscription_type {
            SubscriptionType::Debug => events
                .into_iter()
                .map(|event| RoutedEvent {
                    source_name: event.clone(),
                    scopes: vec![ScopeMetadata::new(AbsoluteMoniker::root()).for_debug()],
                })
                .collect(),
            SubscriptionType::Normal(target_moniker) => {
                let route_result = self.route_events(&target_moniker, &events).await?;
                if route_result.len() != events.len() {
                    let names = events
                        .into_iter()
                        .filter(|event| !route_result.contains_event(&event))
                        .collect();
                    return Err(EventsError::not_available(names).into());
                }
                route_result.to_vec()
            }
        };

        self.subscribe_with_routed_events(&options.sync_mode, events).await
    }

    pub async fn subscribe_with_routed_events(
        &self,
        sync_mode: &SyncMode,
        events: Vec<RoutedEvent>,
    ) -> Result<EventStream, ModelError> {
        // TODO(fxb/48510): get rid of this channel and use FIDL directly.
        let mut event_stream = EventStream::new();

        let mut dispatcher_map = self.dispatcher_map.lock().await;
        for event in &events {
            if EventType::synthesized_only()
                .iter()
                .all(|e| e.to_string() != event.source_name.str())
            {
                let dispatchers = dispatcher_map.entry(event.source_name.clone()).or_insert(vec![]);
                let dispatcher =
                    event_stream.create_dispatcher(sync_mode.clone(), event.scopes.clone());
                dispatchers.push(dispatcher);
            }
        }

        let events = events.into_iter().map(|event| (event.source_name, event.scopes)).collect();
        self.event_synthesizer.spawn_synthesis(event_stream.sender(), events);

        Ok(event_stream)
    }
    // TODO(fxb/48510): get rid of this
    /// Sends the event to all dispatchers and waits to be unblocked by all
    async fn dispatch(&self, event: &ComponentEvent) -> Result<(), ModelError> {
        // Copy the senders so we don't hold onto the sender map lock
        // If we didn't do this, it is possible to deadlock while holding onto this lock.
        // For example,
        // Task A : call dispatch(event1) -> lock on sender map -> send -> wait for responders
        // Task B : call dispatch(event2) -> lock on sender map
        // If task B was required to respond to event1, then this is a deadlock.
        // Neither task can make progress.
        let dispatchers = {
            let mut dispatcher_map = self.dispatcher_map.lock().await;
            if let Some(dispatchers) = dispatcher_map.get_mut(&event.event_type().into()) {
                let mut strong_dispatchers = vec![];
                dispatchers.retain(|dispatcher| {
                    if let Some(dispatcher) = dispatcher.upgrade() {
                        strong_dispatchers.push(dispatcher);
                        true
                    } else {
                        false
                    }
                });
                strong_dispatchers
            } else {
                // There were no senders for this event. Do nothing.
                return Ok(());
            }
        };

        let mut responder_channels = vec![];
        for dispatcher in dispatchers {
            let result = dispatcher.dispatch(event.clone()).await;
            match result {
                Ok(Some(responder_channel)) => {
                    // A future can be canceled if the EventStream was dropped after
                    // a send. We don't crash the system when this happens. It is
                    // perfectly valid for a EventStream to be dropped. That simply
                    // means that the EventStream is no longer interested in future
                    // events. So we force each future to return a success. This
                    // ensures that all the futures can be driven to completion.
                    let responder_channel = async move {
                        trace::duration!("component_manager", "events:wait_for_resume");
                        let _ = responder_channel.await;
                        trace::flow_end!("component_manager", "event", event.id);
                    };
                    responder_channels.push(responder_channel);
                }
                // There's nothing to do if event is outside the scope of the given
                // `EventDispatcher`.
                Ok(None) => (),
                Err(_) => {
                    // A send can fail if the EventStream was dropped. We don't
                    // crash the system when this happens. It is perfectly
                    // valid for a EventStream to be dropped. That simply means
                    // that the EventStream is no longer interested in future
                    // events.
                }
            }
        }

        // Wait until all tasks have used the responder to unblock.
        {
            trace::duration!("component_manager", "events:wait_for_all_resume");
            futures::future::join_all(responder_channels).await;
        }

        Ok(())
    }

    pub async fn route_events(
        &self,
        target_moniker: &AbsoluteMoniker,
        events: &Vec<CapabilityName>,
    ) -> Result<RouteEventsResult, ModelError> {
        let model = self.model.upgrade().ok_or(ModelError::ModelNotAvailable)?;
        let realm = model.look_up_realm(&target_moniker).await?;
        let decl = {
            let state = realm.lock_state().await;
            state.as_ref().expect("route_events: not registered").decl().clone()
        };

        let mut result = RouteEventsResult::new();
        for use_decl in decl.uses {
            match &use_decl {
                UseDecl::Event(event_decl) => {
                    if !events.contains(&event_decl.target_name) {
                        continue;
                    }
                    let (source_name, scope_moniker) =
                        Self::route_event(event_decl, &realm).await?;
                    let scope = ScopeMetadata::new(scope_moniker)
                        .with_filter(EventFilter::new(event_decl.filter.clone()));
                    result.insert(source_name, scope);
                }
                _ => {}
            }
        }

        Ok(result)
    }

    /// Routes an event and returns its source name and scope on success.
    async fn route_event(
        event_decl: &UseEventDecl,
        realm: &Arc<Realm>,
    ) -> Result<(CapabilityName, AbsoluteMoniker), ModelError> {
        routing::route_use_event_capability(&UseDecl::Event(event_decl.clone()), &realm).await.map(
            |source| match source {
                CapabilitySource::Framework {
                    capability: InternalCapability::Event(source_name),
                    scope_moniker,
                } => (source_name, scope_moniker),
                _ => unreachable!(),
            },
        )
    }

    #[cfg(test)]
    async fn dispatchers_per_event_type(&self, event_type: EventType) -> usize {
        let dispatcher_map = self.dispatcher_map.lock().await;
        dispatcher_map
            .get(&event_type.into())
            .map(|dispatchers| dispatchers.len())
            .unwrap_or_default()
    }
}

#[async_trait]
impl Hook for EventRegistry {
    async fn on(self: Arc<Self>, event: &ComponentEvent) -> Result<(), ModelError> {
        self.dispatch(event).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::model::{
            hooks::{Event as ComponentEvent, EventError, EventErrorPayload, EventPayload},
            moniker::AbsoluteMoniker,
            testing::test_helpers::*,
        },
        matches::assert_matches,
    };

    async fn dispatch_fake_event(registry: &EventRegistry) -> Result<(), ModelError> {
        let root_component_url = "test:///root".to_string();
        let event = ComponentEvent::new(
            AbsoluteMoniker::root(),
            Ok(EventPayload::Discovered { component_url: root_component_url }),
        );
        registry.dispatch(&event).await
    }

    async fn dispatch_error_event(registry: &EventRegistry) -> Result<(), ModelError> {
        let root = AbsoluteMoniker::root();
        let event = ComponentEvent::new(
            root.clone(),
            Err(EventError::new(
                &ModelError::instance_not_found(root.clone()),
                EventErrorPayload::Resolved,
            )),
        );
        registry.dispatch(&event).await
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn drop_dispatcher_when_event_stream_dropped() {
        let model = Arc::new(new_test_model("root", Vec::new()));
        let event_registry = EventRegistry::new(Arc::downgrade(&model));

        assert_eq!(0, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        let mut event_stream_a = event_registry
            .subscribe(
                &SubscriptionOptions::new(SubscriptionType::Debug, SyncMode::Async),
                vec![EventType::Discovered.into()],
            )
            .await
            .expect("subscribe succeeds");

        assert_eq!(1, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        let mut event_stream_b = event_registry
            .subscribe(
                &SubscriptionOptions::new(SubscriptionType::Debug, SyncMode::Async),
                vec![EventType::Discovered.into()],
            )
            .await
            .expect("subscribe succeeds");

        assert_eq!(2, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        dispatch_fake_event(&event_registry).await.unwrap();

        // Verify that both EventStreams receive the event.
        assert!(event_stream_a.next().await.is_some());
        assert!(event_stream_b.next().await.is_some());
        assert_eq!(2, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        drop(event_stream_a);

        // EventRegistry won't drop EventDispatchers until an event is dispatched.
        assert_eq!(2, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        dispatch_fake_event(&event_registry).await.unwrap();

        assert!(event_stream_b.next().await.is_some());
        assert_eq!(1, event_registry.dispatchers_per_event_type(EventType::Discovered).await);

        drop(event_stream_b);

        dispatch_fake_event(&event_registry).await.unwrap();
        assert_eq!(0, event_registry.dispatchers_per_event_type(EventType::Discovered).await);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn event_error_dispatch() {
        let model = Arc::new(new_test_model("root", Vec::new()));
        let event_registry = EventRegistry::new(Arc::downgrade(&model));

        assert_eq!(0, event_registry.dispatchers_per_event_type(EventType::Resolved).await);

        let mut event_stream = event_registry
            .subscribe(
                &SubscriptionOptions::new(SubscriptionType::Debug, SyncMode::Async),
                vec![EventType::Resolved.into()],
            )
            .await
            .expect("subscribed to event stream");

        assert_eq!(1, event_registry.dispatchers_per_event_type(EventType::Resolved).await);

        dispatch_error_event(&event_registry).await.unwrap();

        let event = event_stream.next().await.map(|e| e.event).unwrap();

        // Verify that we received the event error.
        assert_matches!(
            event.result,
            Err(EventError {
                source: ModelError::InstanceNotFound { .. },
                event_error_payload: EventErrorPayload::Resolved,
            })
        );
    }
}

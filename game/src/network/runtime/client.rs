//! Client synchronization and replica application.
use super::*;

impl<T: Transport> Runtime<T> {
    pub(super) fn client_event(
        &mut self,
        event: &TransportEvent,
        outcome: BootstrapOutcome,
        store: &mut PieceDataStore,
        interaction: &mut PieceInteraction,
        now: Instant,
    ) -> Result<(), RuntimeFailure> {
        let Role::Client(client) = &mut self.role else {
            unreachable!()
        };
        if client.bootstrap.state() == Some(ConnectionState::Authenticated) {
            let metadata = client.bootstrap.metadata().unwrap();
            self.session = Some(AuthoritySession::new(
                metadata.definition,
                metadata.host,
                metadata.cursor,
            ));
            let sync = ClientSyncRouter::start(
                &mut client.bootstrap,
                client.cached_image.as_deref(),
                client.display_name.clone(),
                now,
            )
            .map_err(RuntimeFailure::sync)?;
            let cached = sync
                .image_ready()
                .then(|| client.cached_image.take())
                .flatten();
            client.cached_image = None;
            client.sync = Some(sync);
            self.status.phase = RuntimePhase::Syncing(SyncPhase::ImageNegotiation);
            if let Some(bytes) = cached {
                self.start_decode(move || Ok(bytes));
            }
        } else if outcome == BootstrapOutcome::Syncing {
            let sync = client.sync.as_mut().ok_or("missing client sync")?;
            let result = sync
                .route(
                    &mut client.bootstrap,
                    &mut self.connections,
                    event,
                    &mut self.transport,
                    &mut SyncReplica {
                        roster: &mut self.roster,
                        replica: &mut client.replica,
                        session: self.session.as_mut().unwrap(),
                        store,
                    },
                    now,
                )
                .map_err(RuntimeFailure::sync)?;
            if let Some(definition) = sync.definition() {
                self.definition = Some(definition.clone());
            }
            self.status.phase = RuntimePhase::Syncing(sync.phase());
            match result {
                ClientSyncOutcome::ImageReady(bytes) => self.start_decode(move || {
                    bytes
                        .into_bytes()
                        .map(Arc::from)
                        .map_err(|e| format!("{e:?}"))
                }),
                ClientSyncOutcome::BaselineInstalled => {
                    self.cursors.reset();
                    self.baseline_installed = true;
                    self.bridge = default();
                    *interaction = default();
                    store.drag = default();
                    store.clear_local_rotation();
                    store.clear_rotation_visual();
                }
                ClientSyncOutcome::Ready => {
                    self.cursors.reset();
                    self.bridge = default();
                    *interaction = default();
                    store.drag = default();
                    store.clear_local_rotation();
                    store.clear_rotation_visual();
                    self.status.local_player = client.bootstrap.assigned_player();
                    self.status.host = Some(self.session.as_ref().unwrap().host());
                    self.status.phase = RuntimePhase::Ready;
                    let session = self.session.as_ref().unwrap();
                    self.presentation.initialize(
                        self.status.local_player.unwrap(),
                        session,
                        store,
                        client.replica.remote_drags(session, store),
                    );
                    client.sync = None;
                }
                _ => {}
            }
        } else if outcome == BootstrapOutcome::Gameplay {
            let player = self.status.local_player.ok_or("missing Ready identity")?;
            let routed = ClientRouter {
                roster: &mut self.roster,
                local_player: player,
                host_connection: client.bootstrap.host_connection(),
                connections: &self.connections,
                replica: &mut client.replica,
                session: self.session.as_mut().unwrap(),
                store,
                definition: self.definition.as_ref(),
            }
            .route(event)
            .map_err(RuntimeFailure::protocol)?;
            if let super::super::client::ClientRouteOutcome::Presence(notification) = &routed {
                self.events.push(notification.clone());
            }
            if let TransportEvent::Message { class, payload, .. } = event {
                match wire::decode_for_class(payload, *class).map_err(RuntimeFailure::protocol)? {
                    WireMessage::AuthorityEvent(envelope) => {
                        let drag = client.replica.remote_drag(
                            self.session.as_ref().unwrap(),
                            store,
                            presentation::event_player(&envelope.event),
                        );
                        self.presentation
                            .event(player, &envelope.event, drag, store);
                        self.bridge
                            .reconcile(player, &envelope.event, interaction, store)
                            .map_err(RuntimeFailure::protocol)?;
                        // Exclusive PreUpdate: canonical apply + prefix retirement
                        // + suffix replay complete before Last/upload/extraction.
                        self.bridge.refresh_prediction(
                            player,
                            self.definition.as_ref(),
                            interaction,
                            store,
                        );
                    }
                    WireMessage::DragUpdate(update)
                        if matches!(routed, super::super::client::ClientRouteOutcome::Drag(_)) =>
                    {
                        self.presentation.delta(
                            update.player,
                            client
                                .replica
                                .remote_drag(self.session.as_ref().unwrap(), store, update.player)
                                .unwrap()
                                .delta,
                        );
                    }
                    WireMessage::Presence(PresenceMessage::PlayerLeft { player, .. }) => {
                        self.cursors.remove(player);
                    }
                    _ => {}
                }
            }
        } else if matches!(
            client.bootstrap.state(),
            Some(
                ConnectionState::TransportConnected
                    | ConnectionState::Authenticating
                    | ConnectionState::Securing
            )
        ) {
            self.status.phase = RuntimePhase::Authenticating;
        }
        Ok(())
    }
}

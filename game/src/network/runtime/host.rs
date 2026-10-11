//! Host synchronization, routing, retained publication and presence.
use super::*;

pub(super) fn publish_presence(
    connections: &SessionConnections,
    transport: &mut dyn Transport,
    source: Option<ConnectionId>,
    event: PresenceMessage,
) -> Result<Vec<(ConnectionId, TransportError)>, RuntimeFailure> {
    let payload = wire::encode(&WireMessage::Presence(event)).map_err(RuntimeFailure::internal)?;
    let mut failures = Vec::new();
    for peer in connections
        .peers()
        .filter(|p| p.player.is_some() && Some(p.connection) != source)
    {
        if let Err(error) = transport.send(peer.connection, MessageClass::Control, &payload) {
            failures.push((peer.connection, error));
        }
    }
    Ok(failures)
}

impl<T: Transport> Runtime<T> {
    /// Cancellation applies once, then retention/publication cannot re-enter it.
    /// Even an internal cancellation/encoding fault must finish roster cleanup.
    pub(super) fn host_disconnected(
        &mut self,
        connection: ConnectionId,
        player: Option<PlayerId>,
        event: &TransportEvent,
        pending: &mut Vec<lifecycle::PendingDisconnect>,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        host.joining.remove(&connection);
        host.sync.disconnect(connection);
        let _ = host.bootstrap.process(
            event,
            &mut self.transport,
            &mut self.connections,
            Instant::now(),
        );
        let mut failure = None;
        if let Some(player) = player {
            let session = self.session.as_mut().unwrap();
            match host.contexts.cancel_replicated(session, store, player) {
                Ok(Some(cancel)) => {
                    self.presentation.event(
                        self.status.local_player.unwrap(),
                        &cancel.authority_event.event,
                        None,
                        store,
                    );
                    if let Err(error) =
                        host.sync
                            .record_authority_event(session, store, &cancel.authority_event)
                    {
                        self.status
                            .set_failure(NetworkFailureKind::Internal, &error);
                    }
                    match (HostRouter {
                        local_player: self.status.local_player.unwrap(),
                        connections: &self.connections,
                        contexts: &mut host.contexts,
                        session,
                        store,
                        definition: self.definition.as_ref(),
                    })
                    .publish_authority_event(&mut self.transport, &cancel.authority_event)
                    {
                        Ok(failures) => pending.extend(failures.into_iter().map(|(id, error)| {
                            (
                                id,
                                HostFailurePolicy::transport(&error).disconnect_reason(),
                                None,
                            )
                        })),
                        Err(error) => {
                            failure = Some(RuntimeFailure::internal(error));
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    failure = Some(RuntimeFailure::internal(error));
                }
            }
            if self.roster.get(player).is_some() {
                let result = self
                    .roster
                    .remove_ready(player)
                    .map_err(RuntimeFailure::internal)
                    .and_then(|left| {
                        publish_presence(&self.connections, &mut self.transport, None, left)
                    });
                match result {
                    Ok(failures) => pending.extend(failures.into_iter().map(|(id, error)| {
                        (
                            id,
                            HostFailurePolicy::transport(&error).disconnect_reason(),
                            None,
                        )
                    })),
                    Err(error) => {
                        failure.get_or_insert(error);
                    }
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
    pub(super) fn host_event(
        &mut self,
        event: &TransportEvent,
        outcome: BootstrapOutcome,
        connection: ConnectionId,
        retained: Option<PlayerId>,
        store: &mut PieceDataStore,
        now: Instant,
    ) -> Result<(), RuntimeFailure> {
        let joined = match self.host_sync_event(event, outcome, connection, store, now) {
            Ok(joined) => joined,
            Err(error) => {
                let policy = error.host_policy();
                let reason = policy.disconnect_reason();
                let Role::Host(host) = &mut self.role else {
                    unreachable!()
                };
                // route() may already have rejected and penalized this sync peer.
                if super::super::lifecycle::is_abuse(reason)
                    && host.sync.phase(connection).is_some()
                {
                    host.sync.penalize(self.transport.origin(connection), now);
                }
                self.note_host_failure(policy, &error);
                return self.disconnect(connection, reason, retained, store);
            }
        };
        if outcome == BootstrapOutcome::Gameplay {
            self.host_gameplay(event, connection, retained, store)?;
        }
        if joined {
            self.host_joined(connection, store)?;
        }
        Ok(())
    }

    pub(super) fn host_maintenance(
        &mut self,
        store: &mut PieceDataStore,
        now: Instant,
    ) -> Result<(), RuntimeFailure> {
        // Low-level expiry removes mappings before returning. Keep Ready identity
        // and let the shared live set own all runtime cleanup, including cancels.
        let retained: std::collections::BTreeMap<_, _> = self
            .connections
            .peers()
            .filter_map(|peer| peer.player.map(|player| (peer.connection, player)))
            .collect();
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        let expired = host
            .bootstrap
            .expire(&mut self.transport, &mut self.connections, now);
        let mut pending = Vec::new();
        for (id, error) in expired {
            let policy = HostFailurePolicy::bootstrap(&error);
            self.note_host_failure(policy, &error);
            pending.push((id, policy.disconnect_reason(), retained.get(&id).copied()));
        }
        self.disconnect_many(pending, store)?;
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        let expired = host.sync.expire(
            &mut SyncHost {
                roster: &mut self.roster,
                bootstrap: &mut host.bootstrap,
                connections: &mut self.connections,
            },
            &mut self.transport,
            now,
        );
        let mut pending = Vec::new();
        for (id, error) in expired {
            let policy = error.host_policy();
            self.note_host_failure(policy, &error);
            pending.push((id, policy.disconnect_reason(), retained.get(&id).copied()));
        }
        self.disconnect_many(pending, store)?;
        self.pump_host_joins(store, now)
    }
    pub(super) fn note_host_failure(
        &mut self,
        policy: HostFailurePolicy,
        error: impl std::fmt::Debug,
    ) {
        match policy {
            HostFailurePolicy::Disconnect(reason) => {
                warn!(?reason, ?error, "host disconnected peer")
            }
            HostFailurePolicy::InternalFailure(_) => {
                self.status.set_failure(NetworkFailureKind::Internal, error);
            }
        }
    }
    pub(super) fn publish(
        &mut self,
        source: Option<ConnectionId>,
        outcome: &HostCommandOutcome,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        if let Some(event) = &outcome.authority_event {
            let Role::Host(host) = &self.role else {
                unreachable!()
            };
            let drag = host.contexts.active_drag(
                self.session.as_ref().unwrap(),
                store,
                presentation::event_player(&event.event),
            );
            self.presentation
                .event(self.status.local_player.unwrap(), &event.event, drag, store);
        }
        if let Some(update) = &outcome.drag_update {
            self.presentation.delta(update.player, update.delta);
        }
        let failures = {
            let Role::Host(host) = &mut self.role else {
                unreachable!()
            };
            HostRouter {
                local_player: self.status.local_player.unwrap(),
                connections: &self.connections,
                contexts: &mut host.contexts,
                session: self.session.as_mut().unwrap(),
                store,
                definition: self.definition.as_ref(),
            }
            .publish(&mut self.transport, source, outcome)
            .map_err(RuntimeFailure::internal)?
        };
        self.disconnect_many(
            failures
                .into_iter()
                .map(|(id, error)| {
                    (
                        id,
                        HostFailurePolicy::transport(&error).disconnect_reason(),
                        None,
                    )
                })
                .collect(),
            store,
        )
    }

    fn host_sync_event(
        &mut self,
        event: &TransportEvent,
        outcome: BootstrapOutcome,
        connection: ConnectionId,
        store: &PieceDataStore,
        now: Instant,
    ) -> Result<bool, super::super::syncing::SyncError> {
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        let session = self.session.as_mut().unwrap();
        let authority = SyncAuthority {
            session,
            store,
            contexts: &host.contexts,
            definition: self.definition.as_ref().unwrap(),
        };
        let result = if outcome == BootstrapOutcome::Syncing {
            host.sync.route(
                &mut SyncHost {
                    roster: &mut self.roster,
                    bootstrap: &mut host.bootstrap,
                    connections: &mut self.connections,
                },
                event,
                &mut self.transport,
                &authority,
                self.image.clone(),
                now,
            )
        } else if host.bootstrap.state(connection) == Some(ConnectionState::Authenticated) {
            host.joining.insert(connection);
            host.sync.start(
                &mut host.bootstrap,
                connection,
                &mut self.transport,
                &authority,
                now,
            )
        } else {
            Ok(())
        };
        result?;
        if host.bootstrap.state(connection) == Some(ConnectionState::Ready) {
            host.joining.remove(&connection);
        }
        Ok(outcome == BootstrapOutcome::Syncing
            && host.bootstrap.state(connection) == Some(ConnectionState::Ready))
    }

    fn host_gameplay(
        &mut self,
        event: &TransportEvent,
        connection: ConnectionId,
        retained: Option<PlayerId>,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        let routed = {
            let Role::Host(host) = &mut self.role else {
                unreachable!()
            };
            let session = self.session.as_mut().unwrap();
            HostRouter {
                local_player: self.status.local_player.unwrap(),
                connections: &self.connections,
                contexts: &mut host.contexts,
                session,
                store,
                definition: self.definition.as_ref(),
            }
            .route_with_sync(event, &mut host.sync)
        };
        match routed {
            Ok(routed) => {
                if let Err(error) = routed.retention {
                    self.status
                        .set_failure(NetworkFailureKind::Internal, &error);
                }
                match routed.gameplay {
                    HostRouteOutcome::Applied(outcome) | HostRouteOutcome::Rejected(outcome) => {
                        self.publish(Some(connection), &outcome, store)?;
                    }
                    HostRouteOutcome::DroppedTransient(_) => {}
                }
            }
            Err(error) => {
                let policy = error.policy();
                self.note_host_failure(policy, &error);
                self.disconnect(connection, policy.disconnect_reason(), retained, store)?;
            }
        }
        Ok(())
    }

    fn host_joined(
        &mut self,
        connection: ConnectionId,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        let player = self.connections.player(connection).unwrap();
        let info = self.roster.get(player).unwrap();
        let joined = PresenceMessage::PlayerJoined {
            revision: self.roster.revision(),
            player: RosterPlayer {
                player,
                display_name: info.display_name.clone(),
            },
        };
        let failures = publish_presence(
            &self.connections,
            &mut self.transport,
            Some(connection),
            joined,
        )?;
        self.disconnect_many(
            failures
                .into_iter()
                .map(|(id, error)| {
                    (
                        id,
                        HostFailurePolicy::transport(&error).disconnect_reason(),
                        None,
                    )
                })
                .collect(),
            store,
        )
    }

    fn pump_host_joins(
        &mut self,
        store: &mut PieceDataStore,
        now: Instant,
    ) -> Result<(), RuntimeFailure> {
        let Role::Host(host) = &mut self.role else {
            unreachable!()
        };
        let mut joining: Vec<_> = host.joining.iter().copied().collect();
        if let Some(next) = host.next_pump {
            let pivot = joining.partition_point(|id| *id < next);
            joining.rotate_left(pivot);
        }
        host.next_pump = joining.get(1).copied().or_else(|| joining.first().copied());
        // Each pump generates at most one message. Give active transfers
        // several rotating passes to use the existing host-wide byte/rate
        // budget even at low frame rates, without pre-generating chunks.
        for (pass, id) in (0..4).flat_map(|pass| joining.iter().copied().map(move |id| (pass, id)))
        {
            // A prior publication failure may have closed another queued join.
            if !self.live.contains(&id) {
                continue;
            }
            let Role::Host(host) = &mut self.role else {
                unreachable!()
            };
            if pass != 0
                && !matches!(
                    host.sync.phase(id),
                    Some(SyncPhase::ImageTransfer | SyncPhase::BaselineTransfer)
                )
            {
                continue;
            }
            let result = {
                let authority = SyncAuthority {
                    session: self.session.as_ref().unwrap(),
                    store,
                    contexts: &host.contexts,
                    definition: self.definition.as_ref().unwrap(),
                };
                if host.sync.phase(id) == Some(SyncPhase::RestartRequired) {
                    host.sync
                        .restart(&host.bootstrap, id, &mut self.transport, &authority, now)
                } else {
                    host.sync
                        .pump(&host.bootstrap, id, &mut self.transport, &authority, now)
                }
            };
            if let Err(error) = result {
                let reason = error.disconnect_reason();
                if super::super::lifecycle::is_abuse(reason) {
                    host.sync.penalize(self.transport.origin(id), now);
                }
                self.note_host_failure(error.host_policy(), &error);
                self.disconnect(id, reason, None, store)?;
            }
        }
        let Role::Host(host) = &self.role else {
            unreachable!()
        };
        self.status.peers = self
            .connections
            .peers()
            .map(|peer| RuntimePeer {
                connection: peer.connection,
                player: peer.player,
                state: host.bootstrap.state(peer.connection),
            })
            .collect();
        Ok(())
    }
}

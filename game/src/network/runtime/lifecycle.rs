//! Shared live-connection ownership and exactly-once cleanup.
use super::*;

pub(super) type PendingDisconnect = (ConnectionId, DisconnectReason, Option<PlayerId>);

impl<T: Transport> Runtime<T> {
    pub(super) fn receive_event(
        &mut self,
        event: &TransportEvent,
        store: &mut PieceDataStore,
    ) -> Result<Option<(ConnectionId, Option<PlayerId>)>, RuntimeFailure> {
        let connection = match *event {
            TransportEvent::Connected { connection }
            | TransportEvent::Disconnected { connection, .. }
            | TransportEvent::ConnectionFailed { connection, .. }
            | TransportEvent::Message { connection, .. } => connection,
        };
        if let TransportEvent::Disconnected { reason, .. }
        | TransportEvent::ConnectionFailed { reason, .. } = *event
        {
            if reason == DisconnectReason::BackendConnectionTimeout
                && matches!(self.role, Role::Client(_))
            {
                self.status
                    .set_failure(NetworkFailureKind::disconnect(reason), reason);
            }
            self.disconnect(connection, reason, None, store)?;
            return Ok(None);
        }
        if matches!(event, TransportEvent::Connected { .. }) {
            self.live.insert(connection);
        }
        Ok(self
            .live
            .contains(&connection)
            .then(|| (connection, self.connections.player(connection))))
    }

    pub(super) fn bootstrap_event(
        &mut self,
        event: &TransportEvent,
        connection: ConnectionId,
        retained: Option<PlayerId>,
        store: &mut PieceDataStore,
        now: Instant,
    ) -> Result<Option<BootstrapOutcome>, RuntimeFailure> {
        let result = match &mut self.role {
            Role::Host(host) => {
                host.bootstrap
                    .update_cursor(self.session.as_ref().unwrap().cursor());
                host.bootstrap
                    .process(event, &mut self.transport, &mut self.connections, now)
            }
            Role::Client(client) => {
                client
                    .bootstrap
                    .process(event, &mut self.transport, &mut self.connections, now)
            }
        };
        match result {
            Ok(outcome) => Ok(Some(outcome)),
            Err(error) => {
                let policy = HostFailurePolicy::bootstrap(&error);
                if matches!(self.role, Role::Host(_)) {
                    self.note_host_failure(policy, &error);
                } else {
                    self.status
                        .set_failure(NetworkFailureKind::bootstrap(&error), &error);
                }
                self.disconnect(connection, policy.disconnect_reason(), retained, store)?;
                Ok(None)
            }
        }
    }
    pub(super) fn disconnect(
        &mut self,
        connection: ConnectionId,
        reason: DisconnectReason,
        retained: Option<PlayerId>,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        self.disconnect_many(vec![(connection, reason, retained)], store)
    }

    /// Live removal owns cancellation even when bootstrap/sync already closed the
    /// connection. Drain every pending peer before returning an internal failure.
    pub(super) fn disconnect_many(
        &mut self,
        mut pending: Vec<PendingDisconnect>,
        store: &mut PieceDataStore,
    ) -> Result<(), RuntimeFailure> {
        let mut failure = None;
        while let Some((connection, reason, retained)) = pending.pop() {
            if !self.live.remove(&connection) {
                continue;
            }
            let player = retained.or_else(|| self.connections.player(connection));
            if let Some(player) = player {
                self.cursors.remove(player);
            }
            let event = TransportEvent::Disconnected { connection, reason };
            let _ = self.transport.close(connection, reason);
            match &mut self.role {
                Role::Host(_) => {
                    if let Err(error) =
                        self.host_disconnected(connection, player, &event, &mut pending, store)
                    {
                        failure.get_or_insert(error);
                    }
                }
                Role::Client(client) => {
                    self.roster.clear();
                    self.cursors.reset();
                    if let Some(sync) = &mut client.sync {
                        sync.invalidate();
                    }
                    client.sync = None;
                    client.cached_image = None;
                    self.decode = None;
                    self.decoded = None;
                    let _ = client.bootstrap.process(
                        &event,
                        &mut self.transport,
                        &mut self.connections,
                        Instant::now(),
                    );
                    // Keep the specific bootstrap/sync diagnostic when the close
                    // reason only says that the protocol was rejected.
                    if self.status.error.is_none() || reason != DisconnectReason::ProtocolViolation
                    {
                        self.status
                            .set_failure(NetworkFailureKind::disconnect(reason), reason);
                    }
                    self.status.phase = RuntimePhase::Disconnected;
                    self.active = false;
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

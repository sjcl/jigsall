use super::*;

impl<T: DirectIpTransport> Runtime<T> {
    pub(super) fn route_cursor(
        &mut self,
        event: &TransportEvent,
        now: Instant,
    ) -> Result<(), String> {
        let TransportEvent::Message {
            connection,
            class,
            payload,
        } = event
        else {
            return Ok(());
        };
        let session = self.session.as_ref().ok_or("missing cursor session")?;
        self.cursors
            .synchronize(session.session_definition().id, session.cursor().epoch);
        match (
            &self.role,
            wire::decode_for_class(payload, *class).map_err(|e| format!("{e:?}"))?,
        ) {
            (Role::Host(host), WireMessage::CursorUpdate(update)) => {
                if host.bootstrap.state(*connection) == Some(ConnectionState::Ready) {
                    if let Some(player) = self.connections.player(*connection) {
                        if self.roster.get(player).is_some() {
                            self.cursors.accept_update(player, update, now);
                        }
                    }
                }
            }
            (Role::Client(client), WireMessage::CursorSnapshot(snapshot)) => {
                if self.status.phase == RuntimePhase::Ready
                    && *connection == client.bootstrap.host_connection()
                    && self.connections.player(*connection) == self.status.host
                {
                    self.cursors.accept_snapshot(
                        snapshot,
                        self.status.local_player.unwrap(),
                        &self.roster,
                        now,
                    );
                }
            }
            _ => return Err("wrong cursor direction".into()),
        }
        Ok(())
    }
    /// Runs after input/camera sampling even while paused. Best-effort sends never
    /// fail gameplay; native lifecycle events still handle connection failures.
    pub(super) fn cursor_frame(&mut self, position: Option<Vec2>, now: Instant) {
        let ready = matches!(self.role, Role::Host(_)) || self.status.phase == RuntimePhase::Ready;
        if !self.active || !ready {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.cursors
            .synchronize(session.session_definition().id, session.cursor().epoch);
        let Some(local) = self.status.local_player else {
            return;
        };
        match &self.role {
            Role::Host(_) => {
                if let Some(update) = self.cursors.sample(position, now) {
                    self.cursors.accept_local(local, update, now);
                }
                self.cursors.expire(now);
                if let Some(snapshot) = self.cursors.snapshot(local, &self.roster, now) {
                    if let Ok(payload) = wire::encode(&WireMessage::CursorSnapshot(snapshot)) {
                        for peer in self.connections.peers().filter(|p| p.player.is_some()) {
                            let _ = self.transport.send_presentation(peer.connection, &payload);
                        }
                    }
                }
            }
            Role::Client(client) => {
                self.cursors.expire_snapshot(now);
                if let Some(update) = self.cursors.sample(position, now) {
                    if let Ok(payload) = wire::encode(&WireMessage::CursorUpdate(update)) {
                        let _ = self
                            .transport
                            .send_presentation(client.bootstrap.host_connection(), &payload);
                    }
                }
            }
        }
    }
}

/// Input sampling does not run while paused; state/transition and current window
/// visibility therefore gate the cached world coordinate independently.
pub(super) fn local_cursor(world: &mut World) -> Option<Vec2> {
    use crate::resources::{AppState, GameSubState, InputState};
    if world
        .get_resource::<crate::resources::LocalGameplayBlocked>()
        .is_some_and(|blocked| blocked.0)
    {
        return None;
    }
    if world.get_resource::<State<GameSubState>>().map(State::get) != Some(&GameSubState::Playing) {
        return None;
    }
    if world.get_resource::<NextState<GameSubState>>().is_some_and(|next| {
        matches!(next, NextState::Pending(state) | NextState::PendingIfNeq(state) if *state != GameSubState::Playing)
    }) || world.get_resource::<NextState<AppState>>().is_some_and(|next| {
        matches!(next, NextState::Pending(state) | NextState::PendingIfNeq(state) if *state != AppState::InGame)
    }) { return None; }
    if world
        .query_filtered::<&Window, With<bevy::window::PrimaryWindow>>()
        .iter(world)
        .any(|window| !window.focused || window.cursor_position().is_none())
    {
        return None;
    }
    world
        .get_resource::<InputState>()
        .filter(|input| input.window_focused)
        .and_then(|input| input.mouse_position)
}

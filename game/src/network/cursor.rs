//! Session-only, world-space presence. No gameplay protocol or piece access.
use crate::{
    players::{PlayerRoster, MAX_ROSTER_PLAYERS},
    resources::remote_cursor::RemoteCursorPresentation,
};
use bevy::prelude::Vec2;
use jigsall_core::{
    session::{AuthorityEpoch, SessionId},
    PlayerId,
};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, Instant},
};

/// All cursor tuning is centralized here; wire ticks are ordering, never time.
pub const CURSOR_INTERVAL: Duration = Duration::from_millis(50);
pub const CURSOR_TIMEOUT: Duration = Duration::from_millis(400);
pub const CURSOR_WORLD_BOUND: f32 = 1_000_000.0;
pub(crate) const CURSOR_SMOOTHING_SECS: f64 = 0.025;
pub(crate) const CURSOR_SETTLE_SECS: f64 = 0.250;
pub(crate) const CURSOR_SETTLE_EPSILON: f64 = 0.001;

pub fn valid_position(position: Vec2) -> bool {
    position.is_finite() && position.abs().max_element() <= CURSOR_WORLD_BOUND
}

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorUpdate {
    pub session: SessionId,
    pub authority_epoch: AuthorityEpoch,
    pub tick: u64,
    pub position: Option<Vec2>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorEntry {
    pub player: PlayerId,
    pub position: Vec2,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CursorSnapshot {
    pub session: SessionId,
    pub authority_epoch: AuthorityEpoch,
    pub sequence: u64,
    #[serde(deserialize_with = "bounded_entries")]
    pub entries: Vec<CursorEntry>,
}
impl CursorSnapshot {
    pub(crate) fn canonical(&self) -> bool {
        self.entries.len() <= MAX_ROSTER_PLAYERS
            && self.entries.windows(2).all(|w| w[0].player < w[1].player)
    }
}
fn bounded_entries<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<CursorEntry>, D::Error> {
    struct Entries;
    impl<'de> de::Visitor<'de> for Entries {
        type Value = Vec<CursorEntry>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded, canonical cursor set")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|n| n > MAX_ROSTER_PLAYERS) {
                return Err(de::Error::custom("too many cursors"));
            }
            let mut entries: Vec<CursorEntry> = Vec::new();
            while let Some(entry) = seq.next_element::<CursorEntry>()? {
                if entries.len() == MAX_ROSTER_PLAYERS
                    || entries
                        .last()
                        .is_some_and(|last| last.player >= entry.player)
                {
                    return Err(de::Error::custom("noncanonical cursor set"));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    d.deserialize_seq(Entries)
}

struct LatestCursor {
    tick: u64,
    position: Option<Vec2>,
    received: Instant,
}
#[derive(Default)]
pub(super) struct CursorPresence {
    scope: Option<(SessionId, AuthorityEpoch)>,
    latest: BTreeMap<PlayerId, LatestCursor>,
    local_tick: Option<u64>,
    snapshot_sequence: Option<u64>,
    received_sequence: Option<u64>,
    last_snapshot: Option<Instant>,
    next_sample: Option<Instant>,
    next_snapshot: Option<Instant>,
    last_snapshot_empty: bool,
    local_visible: bool,
    pub presentation: RemoteCursorPresentation,
}
impl CursorPresence {
    pub fn reset(&mut self) {
        let mut presentation = std::mem::take(&mut self.presentation);
        presentation.clear();
        *self = Self {
            presentation,
            ..Self::default()
        };
    }
    pub fn synchronize(&mut self, session: SessionId, epoch: AuthorityEpoch) {
        if self.scope != Some((session, epoch)) {
            self.reset();
            self.scope = Some((session, epoch));
        }
    }
    pub fn remove(&mut self, player: PlayerId) {
        self.latest.remove(&player);
        self.presentation.remove(player);
    }
    pub fn accept_update(&mut self, player: PlayerId, update: CursorUpdate, now: Instant) {
        self.accept(player, update, now, true);
    }
    pub fn accept_local(&mut self, player: PlayerId, update: CursorUpdate, now: Instant) {
        self.accept(player, update, now, false);
    }
    fn accept(&mut self, player: PlayerId, update: CursorUpdate, now: Instant, present: bool) {
        if self.scope != Some((update.session, update.authority_epoch))
            || self
                .latest
                .get(&player)
                .is_some_and(|v| update.tick <= v.tick)
        {
            return;
        }
        if !self.latest.contains_key(&player) && self.latest.len() == MAX_ROSTER_PLAYERS {
            return;
        }
        let position = update.position.filter(|&p| valid_position(p));
        self.latest.insert(
            player,
            LatestCursor {
                tick: update.tick,
                position,
                received: now,
            },
        );
        if present {
            self.presentation.set_target(player, position);
        }
    }
    pub fn accept_snapshot(
        &mut self,
        snapshot: CursorSnapshot,
        local: PlayerId,
        roster: &PlayerRoster,
        now: Instant,
    ) {
        if self.scope != Some((snapshot.session, snapshot.authority_epoch))
            || self
                .received_sequence
                .is_some_and(|s| snapshot.sequence <= s)
            || !snapshot.canonical()
        {
            return;
        }
        self.received_sequence = Some(snapshot.sequence);
        self.last_snapshot = Some(now);
        self.presentation
            .replace(snapshot.entries.iter().filter_map(|e| {
                (e.player != local && roster.get(e.player).is_some() && valid_position(e.position))
                    .then_some((e.player, e.position))
            }));
    }
    /// At most one sample per frame; hitches never produce catch-up packet bursts.
    pub fn sample(&mut self, position: Option<Vec2>, now: Instant) -> Option<CursorUpdate> {
        let (session, authority_epoch) = self.scope?;
        let position = position.filter(|&p| valid_position(p));
        let hide = self.local_visible && position.is_none();
        if hide {
            self.next_snapshot = None;
        }
        // Hidden players need no periodic update. A lost immediate hide is
        // repaired by host expiry; stationary visible cursors still heartbeat.
        if position.is_none() && !hide {
            return None;
        }
        if !hide && self.next_sample.is_some_and(|next| now < next) {
            return None;
        }
        self.next_sample = Some(now + CURSOR_INTERVAL);
        // Stop sending on exhaustion; never wrap ordering counters.
        let tick = match self.local_tick {
            None => 0,
            Some(t) => t.checked_add(1)?,
        };
        self.local_tick = Some(tick);
        // Track the last sampled message, not unsent visibility fluctuations.
        // An unsent visible frame cannot create another immediate hide packet.
        self.local_visible = position.is_some();
        Some(CursorUpdate {
            session,
            authority_epoch,
            tick,
            position,
        })
    }
    pub fn expire(&mut self, now: Instant) {
        for (&player, latest) in &mut self.latest {
            if latest.position.is_some()
                && now.saturating_duration_since(latest.received) >= CURSOR_TIMEOUT
            {
                latest.position = None;
                self.presentation.remove(player);
            }
        }
    }
    pub fn expire_snapshot(&mut self, now: Instant) {
        if self
            .last_snapshot
            .is_some_and(|last| now.saturating_duration_since(last) >= CURSOR_TIMEOUT)
        {
            self.presentation.clear();
        }
    }
    pub fn snapshot(
        &mut self,
        local: PlayerId,
        roster: &PlayerRoster,
        now: Instant,
    ) -> Option<CursorSnapshot> {
        let (session, authority_epoch) = self.scope?;
        if self.next_snapshot.is_some_and(|next| now < next) {
            return None;
        }
        self.next_snapshot = Some(now + CURSOR_INTERVAL);
        let entries: Vec<_> = self
            .latest
            .iter()
            .filter_map(|(&player, c)| {
                (roster.get(player).is_some())
                    .then_some(c.position)
                    .flatten()
                    .map(|position| CursorEntry { player, position })
            })
            .collect();
        // One empty batch hides the last visible set. If it is lost, client
        // expiry repairs it; visible sets still need periodic heartbeats.
        if entries.is_empty() && self.last_snapshot_empty {
            return None;
        }
        let sequence = match self.snapshot_sequence {
            None => 0,
            Some(s) => s.checked_add(1)?,
        };
        self.snapshot_sequence = Some(sequence);
        self.last_snapshot_empty = entries.is_empty();
        self.presentation.remove(local);
        Some(CursorSnapshot {
            session,
            authority_epoch,
            sequence,
            entries,
        })
    }
}

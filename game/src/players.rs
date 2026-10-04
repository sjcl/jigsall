//! Session-lifetime presence. Never serialized into puzzle checkpoints or saves.
use bevy::prelude::Resource;
use puzzella_core::{PlayerDisplayName, PlayerId};
use serde::{de, Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, fmt};

pub const MAX_ROSTER_PLAYERS: usize = crate::network::lifecycle::MAX_CONNECTIONS + 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterPlayer {
    pub player: PlayerId,
    pub display_name: Option<PlayerDisplayName>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterSnapshot {
    pub revision: u64,
    #[serde(deserialize_with = "bounded_players")]
    pub players: Vec<RosterPlayer>,
}
fn bounded_players<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RosterPlayer>, D::Error> {
    struct PlayersVisitor;
    impl<'de> de::Visitor<'de> for PlayersVisitor {
        type Value = Vec<RosterPlayer>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded player roster")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            if seq.size_hint().is_some_and(|n| n > MAX_ROSTER_PLAYERS) {
                return Err(de::Error::custom("too many roster players"));
            }
            let mut players = Vec::new();
            while let Some(player) = seq.next_element()? {
                if players.len() == MAX_ROSTER_PLAYERS {
                    return Err(de::Error::custom("too many roster players"));
                }
                players.push(player);
            }
            Ok(players)
        }
    }
    d.deserialize_seq(PlayersVisitor)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceMessage {
    PlayerJoined { revision: u64, player: RosterPlayer },
    PlayerLeft { revision: u64, player: PlayerId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub display_name: Option<PlayerDisplayName>,
    pub score: u32,
}
impl From<RosterPlayer> for PlayerInfo {
    fn from(player: RosterPlayer) -> Self {
        Self {
            id: player.player,
            display_name: player.display_name,
            score: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RosterError {
    TooManyPlayers,
    NoncanonicalPlayers,
    MissingHost,
    MissingLocalPlayer,
    InvalidRevision,
    RevisionExhausted,
    DuplicatePlayer,
    UnknownPlayer,
    HostLeft,
}

#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct PlayerRoster {
    revision: u64,
    host: Option<PlayerId>,
    players: BTreeMap<PlayerId, PlayerInfo>,
}
impl PlayerRoster {
    pub fn host_only(host: PlayerId, display_name: Option<PlayerDisplayName>) -> Self {
        Self {
            revision: 0,
            host: Some(host),
            players: BTreeMap::from([(
                host,
                RosterPlayer {
                    player: host,
                    display_name,
                }
                .into(),
            )]),
        }
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn len(&self) -> usize {
        self.players.len()
    }
    pub fn is_empty(&self) -> bool {
        self.players.is_empty()
    }
    pub fn get(&self, player: PlayerId) -> Option<&PlayerInfo> {
        self.players.get(&player)
    }
    pub fn players(&self) -> impl ExactSizeIterator<Item = &PlayerInfo> {
        self.players.values()
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn snapshot(&self) -> RosterSnapshot {
        RosterSnapshot {
            revision: self.revision,
            players: self
                .players()
                .map(|p| RosterPlayer {
                    player: p.id,
                    display_name: p.display_name.clone(),
                })
                .collect(),
        }
    }

    /// Build a complete replacement before mutating either routing or the roster.
    pub fn validated_snapshot(
        snapshot: RosterSnapshot,
        host: PlayerId,
        local: PlayerId,
    ) -> Result<Self, RosterError> {
        if snapshot.players.len() > MAX_ROSTER_PLAYERS {
            return Err(RosterError::TooManyPlayers);
        }
        if snapshot
            .players
            .windows(2)
            .any(|p| p[0].player >= p[1].player)
        {
            return Err(RosterError::NoncanonicalPlayers);
        }
        if !snapshot.players.iter().any(|p| p.player == host) {
            return Err(RosterError::MissingHost);
        }
        if !snapshot.players.iter().any(|p| p.player == local) {
            return Err(RosterError::MissingLocalPlayer);
        }
        // Every non-host entry required at least one join, starting at revision zero.
        if snapshot.revision < (snapshot.players.len() - 1) as u64 {
            return Err(RosterError::InvalidRevision);
        }
        Ok(Self {
            revision: snapshot.revision,
            host: Some(host),
            players: snapshot
                .players
                .into_iter()
                .map(|p| (p.player, p.into()))
                .collect(),
        })
    }
    pub fn install_snapshot(
        &mut self,
        snapshot: RosterSnapshot,
        host: PlayerId,
        local: PlayerId,
    ) -> Result<(), RosterError> {
        *self = Self::validated_snapshot(snapshot, host, local)?;
        Ok(())
    }
    fn next_revision(&self) -> Result<u64, RosterError> {
        self.revision
            .checked_add(1)
            .ok_or(RosterError::RevisionExhausted)
    }
    pub(crate) fn prepare_join(&self, player: RosterPlayer) -> Result<Self, RosterError> {
        let mut snapshot = self.snapshot();
        snapshot.revision = self.next_revision()?;
        if self.players.contains_key(&player.player) {
            return Err(RosterError::DuplicatePlayer);
        }
        snapshot.players.push(player);
        snapshot.players.sort_by_key(|p| p.player);
        Self::validated_snapshot(
            snapshot,
            self.host.ok_or(RosterError::MissingHost)?,
            self.host.unwrap(),
        )
    }
    /// Reliable events must be contiguous; all checks precede the mutation.
    pub fn apply_presence(&mut self, event: PresenceMessage) -> Result<(), RosterError> {
        let next = self.next_revision()?;
        if self.host.is_none() {
            return Err(RosterError::MissingHost);
        }
        match event {
            PresenceMessage::PlayerJoined { revision, player } => {
                if revision != next {
                    return Err(RosterError::InvalidRevision);
                }
                if self.players.contains_key(&player.player) {
                    return Err(RosterError::DuplicatePlayer);
                }
                if self.len() == MAX_ROSTER_PLAYERS {
                    return Err(RosterError::TooManyPlayers);
                }
                self.players.insert(player.player, player.into());
            }
            PresenceMessage::PlayerLeft { revision, player } => {
                if revision != next {
                    return Err(RosterError::InvalidRevision);
                }
                if Some(player) == self.host {
                    return Err(RosterError::HostLeft);
                }
                if !self.players.contains_key(&player) {
                    return Err(RosterError::UnknownPlayer);
                }
                self.players.remove(&player);
            }
        }
        self.revision = next;
        Ok(())
    }
    pub(crate) fn remove_ready(
        &mut self,
        player: PlayerId,
    ) -> Result<PresenceMessage, RosterError> {
        let event = PresenceMessage::PlayerLeft {
            revision: self.next_revision()?,
            player,
        };
        self.apply_presence(event.clone())?;
        Ok(event)
    }
}

#[cfg(test)]
mod tests;

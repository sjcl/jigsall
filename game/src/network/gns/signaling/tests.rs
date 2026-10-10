use super::*;
use std::time::Duration;
fn peer(n: u8) -> PeerId {
    PeerId::from_bytes([n; 16])
}
fn key(n: u8) -> SignalKey {
    SignalKey::Unverified(peer(n))
}
fn route(n: u8) -> RouteOrigin {
    RouteOrigin::from_authenticated_route([1; 16], [2; 16], [n; 16])
}

#[test]
fn flooding_one_peer_cannot_fill_the_mailbox_or_starve_another_peer() {
    let now = Instant::now();
    let mut q = Queue::default();
    for _ in 0..MAX_SIGNALS_PER_ROUTE {
        q.push(peer(1), key(1), &[0, 255, 7], now).unwrap();
    }
    assert_eq!(
        q.push(peer(1), key(1), &[1], now),
        Err(TransportError::Backpressure)
    );
    q.push(peer(2), key(2), &[17], now).unwrap();
    assert_eq!(q.pop(now, true).unwrap().1.peer, peer(1));
    let other = q.pop(now, true).unwrap().1;
    assert_eq!((other.peer, other.payload), (peer(2), vec![17]));
    while q.pop(now, false).is_some() {}
    assert_eq!(
        q.push(peer(1), key(1), &[1], now),
        Err(TransportError::Backpressure)
    );
    q.push(peer(1), key(1), &[1], now + Duration::from_secs(1))
        .unwrap();
}
#[test]
fn many_peer_ids_share_one_authenticated_route_budget_and_fair_turn() {
    let now = Instant::now();
    let mut q = Queue::default();
    for n in 0..MAX_SIGNALS_PER_ROUTE as u8 {
        q.push(peer(n), SignalKey::Route(route(1)), &[n], now)
            .unwrap();
    }
    assert_eq!(
        q.push(peer(99), SignalKey::Route(route(1)), &[1], now),
        Err(TransportError::Backpressure)
    );
    q.push(peer(100), SignalKey::Route(route(2)), &[42], now)
        .unwrap();
    q.pop(now, true).unwrap();
    assert_eq!(q.pop(now, true).unwrap().1.peer, peer(100));
}
#[test]
fn exhausted_global_budget_defers_without_dropping_or_resetting_the_fair_cursor() {
    let now = Instant::now();
    let mut q = Queue {
        global: Bucket::per_second(1, 1, now),
        ..Queue::default()
    };
    q.push(peer(1), key(1), &[1], now).unwrap();
    q.push(peer(2), key(2), &[2], now).unwrap();
    assert_eq!(q.pop(now, true).unwrap().1.peer, peer(1));
    assert!(q.pop(now, true).is_none());
    assert_eq!(q.count, 1);
    assert_eq!(
        q.pop(now + Duration::from_secs(1), true).unwrap().1.peer,
        peer(2)
    );
}
#[test]
fn pending_inbound_query_preserves_deferred_signals_and_distinguishes_route_peers() {
    let endpoint = SignalingEndpoint::routed();
    endpoint.authorize_peer(peer(1), route(1)).unwrap();
    endpoint.authorize_peer(peer(2), route(1)).unwrap();
    endpoint.receive(peer(1), &[1]).unwrap();
    endpoint.0.lock().unwrap().inbound.global = Bucket::per_second(0, 1, Instant::now());
    assert!(endpoint.pop_inbound().is_none());
    assert!(endpoint.has_pending_inbound(peer(1)));
    assert!(!endpoint.has_pending_inbound(peer(2)));
    assert!(!endpoint.has_pending_inbound(peer(3)));
    endpoint.0.lock().unwrap().inbound.global = Bucket::per_second(1, 1, Instant::now());
    assert_eq!(endpoint.pop_inbound().unwrap().peer, peer(1));
    assert!(!endpoint.has_pending_inbound(peer(1)));
}
#[test]
fn byte_count_size_and_history_bounds_hold_under_many_identities() {
    let now = Instant::now();
    let mut q = Queue::default();
    assert_eq!(
        q.push(peer(1), key(1), &vec![0; MAX_SIGNAL_BYTES + 1], now),
        Err(TransportError::PayloadTooLarge)
    );
    let large = vec![7; MAX_SIGNAL_BYTES];
    for n in 0..8 {
        q.push(peer(n), key(n), &large, now).unwrap();
        q.push(peer(n), key(n), &large, now).unwrap();
    }
    assert_eq!(q.bytes, MAX_SIGNAL_QUEUE_BYTES);
    assert_eq!(
        q.push(peer(99), key(99), &[1], now),
        Err(TransportError::Backpressure)
    );
    let mut q = Queue::default();
    for n in 0..8 {
        for _ in 0..MAX_SIGNALS_PER_ROUTE {
            q.push(peer(n), key(n), &[1], now).unwrap();
        }
    }
    assert_eq!(q.count, MAX_QUEUED_SIGNALS);
    assert_eq!(
        q.push(peer(99), key(99), &[1], now),
        Err(TransportError::Backpressure)
    );
    let mut q = Queue::default();
    for n in 0..MAX_ORIGINS {
        q.push(peer(n as u8), key(n as u8), &[1], now).unwrap();
        q.pop(now, false).unwrap();
    }
    let extra = SignalKey::Route(route(42));
    q.push(peer(42), extra, &[1], now).unwrap();
    assert!(q.routes.len() <= MAX_QUEUED_SIGNALS);
    assert!(q.retired.len() <= MAX_RETIRED_RATES);
    q.push(peer(42), extra, &[1], now + ORIGIN_TTL).unwrap();
}
#[test]
fn trusted_route_binding_rejects_unknown_envelopes_and_revocation_purges_queues() {
    let q = SignalingEndpoint::routed();
    assert_eq!(
        q.receive(peer(1), &[1]),
        Err(TransportError::ProtocolViolation)
    );
    q.authorize_peer(peer(1), route(1)).unwrap();
    assert_eq!(
        q.authorize_peer(peer(1), route(2)),
        Err(TransportError::ProtocolViolation)
    );
    assert_eq!(q.origin(peer(1)).unwrap(), Some(Origin::Route(route(1))));
    q.receive(peer(1), &[0, 255, 7]).unwrap();
    assert!(q.send(peer(1), &[9]));
    q.revoke_peer(peer(1));
    assert!(q.pop_inbound().is_none() && q.pop_outbound().is_none());
    assert_eq!(
        q.receive(peer(1), &[1]),
        Err(TransportError::ProtocolViolation)
    );
    q.authorize_peer(peer(1), route(1)).unwrap();
    q.receive(peer(1), &[1]).unwrap();
    let stale = q.pop_inbound().unwrap();
    q.revoke_peer(peer(1));
    q.authorize_peer(peer(1), route(2)).unwrap();
    assert_eq!(stale.origin, Some(Origin::Route(route(1))));
    assert_ne!(stale.origin, q.origin(peer(1)).unwrap());
    q.close();
    assert_eq!(
        q.authorize_peer(peer(1), route(1)),
        Err(TransportError::NotConnected)
    );
    let local = SignalingEndpoint::default();
    assert_eq!(local.origin(peer(1)).unwrap(), None);
    assert_eq!(
        local.authorize_peer(peer(1), route(1)),
        Err(TransportError::ProtocolViolation)
    );
}
#[test]
fn in_memory_delivery_preserves_opaque_bytes_and_closed_mailboxes_do_not_revive() {
    let a = SignalingEndpoint::default();
    let b = SignalingEndpoint::default();
    let mut relay = InMemorySignaling::default();
    relay.register(peer(1), a.clone()).unwrap();
    relay.register(peer(2), b.clone()).unwrap();
    assert!(a.send(peer(2), &[0, 255, 7]));
    relay.poll();
    let s = b.pop_inbound().unwrap();
    assert_eq!((s.peer, s.payload), (peer(1), vec![0, 255, 7]));
    b.close();
    assert!(!b.send(peer(1), &[1]));
    assert_eq!(b.receive(peer(1), &[1]), Err(TransportError::NotConnected));
}

#[test]
fn one_account_cannot_fill_the_authorized_peer_table() {
    let q = SignalingEndpoint::routed();
    for n in 0..MAX_PEERS_PER_ROUTE as u8 {
        q.authorize_peer(peer(n), route(1)).unwrap();
    }
    assert_eq!(
        q.authorize_peer(peer(99), route(1)),
        Err(TransportError::Capacity)
    );
    q.authorize_peer(peer(100), route(2)).unwrap();
    q.revoke_peer(peer(0));
    q.authorize_peer(peer(99), route(1)).unwrap();
}

#[test]
fn history_reclamation_preserves_queued_signals_and_shared_abuse_keys_keep_separate_routes() {
    let now = Instant::now();
    let mut q = Queue::default();
    for n in 0..MAX_ORIGINS {
        q.push(peer(n as u8), key(n as u8), &[1], now).unwrap();
        q.pop(now, false).unwrap();
    }
    let now = now + Duration::from_secs(1);
    q.push(peer(7), key(7), &[77], now).unwrap();
    let a = route(1).with_abuse_key([9; 16]);
    let b = route(2).with_abuse_key([9; 16]);
    q.push(peer(10), SignalKey::Route(a), &[10], now).unwrap();
    q.push(peer(11), SignalKey::Route(b), &[11], now).unwrap();
    assert!(q.routes.len() <= MAX_QUEUED_SIGNALS);
    assert!(q.retired.len() <= MAX_RETIRED_RATES);
    assert!(q.routes.contains_key(&key(7)));
    assert_eq!(q.pop(now, false).unwrap().1.payload, vec![77]);
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(10));
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(11));
}

fn numbered(n: u32) -> (PeerId, SignalKey) {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&n.to_le_bytes());
    let peer = PeerId::from_bytes(bytes);
    (peer, SignalKey::Unverified(peer))
}
#[test]
fn exhausted_route_survives_forced_collisions_and_thousands_of_queue_reclamations() {
    let now = Instant::now();
    let mut q = Queue::default();
    q.retired.force_collisions();
    let (a, ka) = numbered(1);
    for _ in 0..MAX_SIGNALS_PER_ROUTE {
        q.push(a, ka, &[1], now).unwrap();
    }
    while q.pop(now, false).is_some() {}
    for n in 1000..11000 {
        let (peer, key) = numbered(n);
        q.push(peer, key, &[2], now).unwrap();
        q.pop(now, false); // May defer when the miss-only budget is exhausted.
        q.revoke(peer, now);
        assert!(q.routes.len() <= MAX_QUEUED_SIGNALS);
        assert!(q.retired.len() <= MAX_RETIRED_RATES);
        assert_eq!((q.count, q.bytes), (0, 0));
    }
    assert_eq!(q.push(a, ka, &[1], now), Err(TransportError::Backpressure));
    let (b, kb) = numbered(2);
    for i in 0..MAX_SIGNALS_PER_ROUTE {
        q.push(b, kb, &[i as u8], now).unwrap();
    }
    assert!(q.pop(now, false).is_none());
    assert_eq!(q.count, MAX_SIGNALS_PER_ROUTE);
    for i in 0..MAX_SIGNALS_PER_ROUTE {
        assert_eq!(
            q.pop(now + Duration::from_secs(1), false)
                .unwrap()
                .1
                .payload,
            vec![i as u8]
        );
    }
    assert_eq!((q.count, q.bytes), (0, 0));
}
#[test]
fn colliding_route_does_not_inherit_credit_and_revoke_does_not_refund_it() {
    let now = Instant::now();
    let mut q = Queue::default();
    q.retired.force_collisions();
    for _ in 0..MAX_SIGNALS_PER_ROUTE {
        q.push(peer(1), key(1), &[1], now).unwrap();
    }
    q.revoke(peer(1), now);
    assert_eq!(
        q.push(peer(1), key(1), &[1], now),
        Err(TransportError::Backpressure)
    );
    for _ in 0..MAX_SIGNALS_PER_ROUTE {
        q.push(peer(2), key(2), &[2], now).unwrap();
    }
    assert_eq!(q.count, MAX_SIGNALS_PER_ROUTE);
    assert!(!q.retired.get_mut(key(1)).unwrap().bucket.take(1, now));
}
#[test]
fn fallback_defers_fairly_preserves_fifo_bytes_and_does_not_block_remembered_routes() {
    let now = Instant::now();
    let mut q = Queue::default();
    q.push(peer(1), key(1), &[1], now).unwrap();
    q.pop(now, false).unwrap();
    q.untracked_until = Some(now + RATE_REFILL);
    q.untracked = Bucket::per_second(1, 1, now);
    for p in [peer(2), peer(3)] {
        let key = SignalKey::Unverified(p);
        q.push(p, key, &[20], now).unwrap();
        q.push(p, key, &[21], now).unwrap();
    }
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(2));
    let count = q.count;
    let bytes = q.bytes;
    assert!(q.pop(now, false).is_none());
    assert_eq!((q.count, q.bytes), (count, bytes));
    q.push(peer(1), key(1), &[10], now).unwrap();
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(1));
    let later = now + Duration::from_secs(1);
    assert_eq!(q.pop(later, false).unwrap().1.peer, peer(3));
    assert_eq!(
        q.pop(later + Duration::from_secs(1), false)
            .unwrap()
            .1
            .payload,
        vec![21]
    );
    assert_eq!(
        q.pop(later + Duration::from_secs(2), false)
            .unwrap()
            .1
            .payload,
        vec![21]
    );
    assert_eq!((q.count, q.bytes), (0, 0));
}
#[test]
fn freed_history_slot_does_not_bypass_recently_lost_rate_debt() {
    let now = Instant::now();
    let mut q = Queue::default();
    for n in 0..MAX_RETIRED_RATES as u32 {
        let (p, k) = numbered(n);
        q.push(p, k, &[1], now).unwrap();
        q.pop(now, false).unwrap();
    }
    let (lost, klost) = numbered(999);
    q.push(lost, klost, &[1], now).unwrap();
    q.pop(now, false).unwrap();
    // Restoring a remembered route frees a retired slot, without elapsed time.
    let (known, kknown) = numbered(0);
    q.push(known, kknown, &[1], now).unwrap();
    q.push(lost, klost, &[1], now).unwrap();
    assert!(q.routes[&klost].untracked);
    assert!(!q.routes[&kknown].untracked);
}

#[test]
fn repeated_refill_churn_and_revoke_stay_within_previous_memory_ceiling() {
    #[allow(dead_code)]
    struct OldRouteQueue {
        messages: VecDeque<OutboundSignal>,
        bytes: usize,
        rate: Bucket,
        touched: Instant,
    }
    let key_size = std::mem::size_of::<SignalKey>();
    let new_metadata = MAX_QUEUED_SIGNALS * (key_size + std::mem::size_of::<RouteQueue>())
        + MAX_RETIRED_RATES * std::mem::size_of::<(u64, SignalKey, RateHistory)>()
        + std::mem::size_of::<HistoryCache<SignalKey, RateHistory>>()
        + std::mem::size_of::<Bucket>()
        + std::mem::size_of::<Option<Instant>>();
    assert!(new_metadata <= MAX_ORIGINS * (key_size + std::mem::size_of::<OldRouteQueue>()));
    let start = Instant::now();
    let mut q = Queue::default();
    for round in 0..1000u32 {
        let now = start + Duration::from_secs(round as u64);
        for n in 0..10 {
            let (p, k) = numbered(round * 10 + n);
            q.push(p, k, &[1, 2], now).unwrap();
            if n % 2 == 0 {
                q.revoke(p, now);
            } else {
                q.pop(now, false).unwrap();
            }
        }
        assert!(q.retired.len() <= MAX_RETIRED_RATES);
        assert!(q.routes.len() <= MAX_QUEUED_SIGNALS);
        assert_eq!((q.count, q.bytes), (0, 0));
        assert!(q.active.is_empty());
    }
}

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
    assert_eq!(q.routes.len(), MAX_ORIGINS);
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
    q.push(peer(7), key(7), &[77], now).unwrap();
    let a = route(1).with_abuse_key([9; 16]);
    let b = route(2).with_abuse_key([9; 16]);
    q.push(peer(10), SignalKey::Route(a), &[10], now).unwrap();
    q.push(peer(11), SignalKey::Route(b), &[11], now).unwrap();
    assert_eq!(q.routes.len(), MAX_ORIGINS);
    assert!(q.routes.contains_key(&key(7)));
    assert_eq!(q.pop(now, false).unwrap().1.payload, vec![77]);
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(10));
    assert_eq!(q.pop(now, false).unwrap().1.peer, peer(11));
}

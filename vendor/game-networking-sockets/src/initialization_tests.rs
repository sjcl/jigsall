use super::*;

#[test]
fn identity_initialization_is_single_shot_concurrent_and_rejects_conflicts() {
    let mut identity = SteamNetworkingIdentity::default();
    // SAFETY: initialized 16-byte input, copied by the native identity helper.
    unsafe {
        SteamAPI_SteamNetworkingIdentity_SetGenericBytes(
            &mut identity,
            [9u8; 16].as_ptr().cast(),
            16,
        );
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| GnsGlobal::get_with_identity(&identity).unwrap()))
            .collect();
        let globals: Vec<_> = handles.into_iter().map(|t| t.join().unwrap()).collect();
        for global in &globals {
            assert!(std::ptr::eq(*global, globals[0]));
        }
        assert!(std::ptr::eq(GnsGlobal::get().unwrap(), globals[0]));
    });
    let mut different = SteamNetworkingIdentity::default();
    unsafe {
        SteamAPI_SteamNetworkingIdentity_SetGenericBytes(
            &mut different,
            [8u8; 16].as_ptr().cast(),
            16,
        );
    }
    assert!(matches!(
        GnsGlobal::get_with_identity(&different),
        Err(GnsError::Init(_))
    ));
    assert!(GnsGlobal::get_with_identity(&identity).is_ok());
    assert_eq!(INIT_CALLS.load(Ordering::Relaxed), 1);
}

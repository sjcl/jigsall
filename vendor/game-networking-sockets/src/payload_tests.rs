use super::*;
use std::sync::atomic::AtomicUsize;
use std::{mem::ManuallyDrop, ptr};

fn with_message(data: *mut c_void, size: i32, check: impl FnOnce(&GnsNetworkMessage<ToReceive>)) {
    let mut native = ISteamNetworkingMessage {
        m_pData: data,
        m_cbSize: size,
        ..Default::default()
    };
    // Stack fixture: suppress native Release, which requires a native allocation.
    let message = ManuallyDrop::new(GnsNetworkMessage(&mut native, PhantomData));
    check(&message);
}

#[test]
fn payload_zero_size_accepts_null_and_nonnull_without_constructing_a_slice() {
    for data in [
        ptr::null_mut(),
        ptr::NonNull::<u8>::dangling().as_ptr().cast(),
    ] {
        with_message(data, 0, |message| {
            assert_eq!(message.try_payload().unwrap(), &[]);
            assert_eq!(message.payload(), &[]);
        });
    }
}

#[test]
fn payload_negative_sizes_are_rejected_before_pointer_access() {
    for size in [-1, i32::MIN] {
        for data in [
            ptr::null_mut(),
            ptr::NonNull::<u8>::dangling().as_ptr().cast(),
        ] {
            with_message(data, size, |message| {
                assert_eq!(message.try_payload(), Err(GnsError::InvalidMessagePayload));
            });
        }
    }
}

#[test]
fn payload_nonempty_null_is_rejected() {
    for size in [1, k_cbMaxSteamNetworkingSocketsMessageSizeSend, i32::MAX] {
        with_message(ptr::null_mut(), size, |message| {
            assert_eq!(message.try_payload(), Err(GnsError::InvalidMessagePayload));
        });
    }
}

#[test]
fn payload_nonempty_preserves_bytes_and_borrowed_pointer() {
    let mut bytes = [0, 1, 0xff, 2];
    with_message(bytes.as_mut_ptr().cast(), bytes.len() as i32, |message| {
        let payload = message.try_payload().unwrap();
        assert_eq!(payload, &bytes);
        assert_eq!(payload.as_ptr(), bytes.as_ptr());
        assert_eq!(message.payload(), &bytes);
    });
}

#[test]
fn payload_valid_allocation_above_native_send_limit_remains_inspectable() {
    // Receive limits can exceed the send default; owned failed sends also
    // remain inspectable. Application size policies belong to the adapters.
    let mut bytes = vec![0x42; k_cbMaxSteamNetworkingSocketsMessageSizeSend as usize + 1];
    with_message(bytes.as_mut_ptr().cast(), bytes.len() as i32, |message| {
        assert_eq!(message.try_payload().unwrap(), bytes);
        assert_eq!(message.payload(), bytes);
    });
}

#[test]
#[should_panic(expected = "invalid native message payload")]
fn payload_infallible_accessor_panics_safely_on_invalid_metadata() {
    with_message(ptr::null_mut(), 1, |message| {
        message.payload();
    });
}

#[derive(Default)]
struct PayloadObservations {
    drops: AtomicUsize,
    reclaimed_len: AtomicUsize,
}

// Metadata-only fixture: the synthetic length is never used to read bytes or
// send a message. This exercises large lengths without a multi-GiB allocation.
struct LengthProbe {
    len: usize,
    observations: Arc<PayloadObservations>,
}

impl Drop for LengthProbe {
    fn drop(&mut self) {
        self.observations.drops.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: from_raw reconstructs exactly the boxed probe that into_raw owns.
unsafe impl Payload for LengthProbe {
    fn into_raw(self) -> (*mut u8, usize) {
        let len = self.len;
        (Box::into_raw(Box::new(self)).cast(), len)
    }

    unsafe fn from_raw(ptr: *mut u8, len: usize) -> Self {
        // SAFETY: only into_raw creates the pointer used by this fixture.
        let probe = *unsafe { Box::from_raw(ptr.cast::<Self>()) };
        probe
            .observations
            .reclaimed_len
            .store(len, Ordering::Relaxed);
        probe
    }
}

extern "C" fn release_probe_message(message: *mut ISteamNetworkingMessage) {
    // SAFETY: the stack message and counter outlive the wrapper's release.
    let message = unsafe { &mut *message };
    let releases = unsafe { &*(message.m_nUserData as usize as *const AtomicUsize) };
    releases.fetch_add(1, Ordering::Relaxed);
    if let Some(free_data) = message.m_pfnFreeData {
        // SAFETY: the constructor installs this callback with its owned probe.
        unsafe { free_data(message) };
    }
}

fn check_outbound_length(len: usize, rejected: bool) {
    let observations = Arc::new(PayloadObservations::default());
    let releases = AtomicUsize::new(0);
    let mut native = ISteamNetworkingMessage {
        m_nUserData: &releases as *const AtomicUsize as usize as i64,
        m_pfnRelease: Some(release_probe_message),
        ..Default::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(GnsNetworkMessage::new(
            &mut native,
            GnsConnection::default(),
            SendFlags::RELIABLE,
            LengthProbe {
                len,
                observations: Arc::clone(&observations),
            },
        ));
    }));

    assert_eq!(result.is_err(), rejected, "length {len}");
    assert_eq!(observations.reclaimed_len.load(Ordering::Relaxed), len);
    assert_eq!(observations.drops.load(Ordering::Relaxed), 1);
    assert_eq!(releases.load(Ordering::Relaxed), 1);
    if rejected {
        assert!(native.m_pData.is_null());
        assert_eq!(native.m_cbSize, 0);
        assert!(native.m_pfnFreeData.is_none());
    } else {
        assert_eq!(native.m_cbSize, i32::try_from(len).unwrap());
    }
}

#[test]
fn outbound_payload_preserves_representable_lengths_and_releases_once() {
    for len in [0, 1, i32::MAX as usize] {
        check_outbound_length(len, false);
    }
}

#[test]
fn outbound_payload_rejects_oversized_lengths_and_reclaims_original_ownership() {
    for len in [i32::MAX as usize + 1, u32::MAX as usize, usize::MAX] {
        check_outbound_length(len, true);
    }
    if let Some(len) = 1usize.checked_shl(32) {
        // A narrowing cast wraps this length to zero on 64-bit hosts.
        check_outbound_length(len, true);
    }
}

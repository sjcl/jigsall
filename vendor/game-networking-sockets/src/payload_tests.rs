use super::*;
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

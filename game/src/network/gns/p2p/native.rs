//! The only raw/unsafe P2P boundary. Uses the pinned wrapper's re-exported sys
//! bindings, so initialization, ABI and native library are shared with Direct IP.
#![deny(unsafe_op_in_unsafe_fn)]
use super::{
    signaling::{PeerId, SignalingEndpoint, MAX_SIGNAL_BYTES},
    IceConfig, TurnServer,
};
use crate::network::{
    lifecycle::{Admission, Bucket},
    transport::{MessageClass, Origin, TransportError},
};
use ::gns::{sys::*, GnsGlobal};
use std::{
    ffi::{c_void, CString},
    ptr,
    sync::OnceLock,
    time::Instant,
};

fn failure() -> TransportError {
    TransportError::Backend("GNS P2P operation failed".into())
}
fn check(result: EResult) -> Result<(), TransportError> {
    if result == EResult::k_EResultOK {
        Ok(())
    } else {
        Err(failure())
    }
}

static IDENTITY: OnceLock<Result<PeerId, TransportError>> = OnceLock::new();
pub(in crate::network::gns) fn global() -> Result<&'static GnsGlobal, TransportError> {
    IDENTITY
        .get_or_init(|| {
            let peer = PeerId::random();
            let identity = identity(peer);
            // The patched pinned wrapper supplies identity to its sole native
            // initializer, or rejects a conflicting initialized identity.
            GnsGlobal::get_with_identity(&identity)
                .map_err(|e| TransportError::Backend(e.to_string()))?;
            let mut actual = SteamNetworkingIdentity::default();
            // Fail closed if another user of gns initialized a different identity.
            if !unsafe { SteamAPI_ISteamNetworkingSockets_GetIdentity(interface(), &mut actual) }
                || self::peer(&actual) != Some(peer)
            {
                return Err(failure());
            }
            Ok(peer)
        })
        .as_ref()
        .map_err(Clone::clone)?;
    GnsGlobal::get().map_err(|e| TransportError::Backend(e.to_string()))
}
pub(super) fn local_peer() -> Result<PeerId, TransportError> {
    global()?;
    IDENTITY.get().expect("initialized identity").clone()
}
fn interface() -> *mut ISteamNetworkingSockets {
    // SAFETY: every owner is created after global(); its singleton is static.
    unsafe { SteamAPI_SteamNetworkingSockets_v009() }
}
fn identity(peer: PeerId) -> SteamNetworkingIdentity {
    let mut identity = SteamNetworkingIdentity::default();
    let bytes = peer.to_bytes();
    // SAFETY: GNS copies exactly 16 initialized bytes into a valid identity.
    unsafe {
        SteamAPI_SteamNetworkingIdentity_SetGenericBytes(&mut identity, bytes.as_ptr().cast(), 16);
    }
    identity
}
fn peer(identity: &SteamNetworkingIdentity) -> Option<PeerId> {
    let mut identity = *identity;
    let mut len = 0;
    // SAFETY: pointer refers into the stack copy, valid through the byte copy.
    let data = unsafe { SteamAPI_SteamNetworkingIdentity_GetGenericBytes(&mut identity, &mut len) };
    if len != 16 || data.is_null() {
        return None;
    }
    let mut bytes = [0; 16];
    unsafe {
        ptr::copy_nonoverlapping(data, bytes.as_mut_ptr(), 16);
    }
    Some(PeerId::from_bytes(bytes))
}

struct SendContext {
    peer: PeerId,
    mailbox: SignalingEndpoint,
}
unsafe extern "C" fn send_signal(
    ctx: *mut c_void,
    _: HSteamNetConnection,
    _: *const SteamNetConnectionInfo_t,
    data: *const c_void,
    len: i32,
) -> bool {
    // SAFETY: GNS owns the boxed SendContext until Release, and serializes
    // Release against its callbacks. It may call SendSignal on its service
    // thread. This callback reads immutable context and uses only a mailbox
    // lock, with no GNS calls or externally supplied Rust closures.
    if ctx.is_null() || data.is_null() || len <= 0 || len as usize > MAX_SIGNAL_BYTES {
        return false;
    }
    let ctx = unsafe { &*ctx.cast::<SendContext>() };
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) };
    ctx.mailbox.send(ctx.peer, bytes)
}
unsafe extern "C" fn release_signal(ctx: *mut c_void) {
    // SAFETY: exactly one Box was transferred to each native signaling object.
    // Release is called exactly once, including failed Connect calls. No
    // references escape SendSignal; mailbox Arcs may outlive the backend.
    unsafe {
        drop(Box::from_raw(ctx.cast::<SendContext>()));
    }
}
fn signaling(peer: PeerId, mailbox: SignalingEndpoint) -> *mut ISteamNetworkingConnectionSignaling {
    let ctx = Box::into_raw(Box::new(SendContext { peer, mailbox }));
    // SAFETY: the native flat adapter owns ctx until release_signal, which
    // destroys it. The caller transfers the adapter immediately to GNS.
    unsafe {
        SteamAPI_ISteamNetworkingSockets_CreateCustomSignaling(
            ctx.cast(),
            Some(send_signal),
            Some(release_signal),
        )
    }
}

fn int_option(value: ESteamNetworkingConfigValue, data: i32) -> SteamNetworkingConfigValue_t {
    SteamNetworkingConfigValue_t {
        m_eValue: value,
        m_eDataType: ESteamNetworkingConfigDataType::k_ESteamNetworkingConfig_Int32,
        m_val: SteamNetworkingConfigValue_t__bindgen_ty_1 { m_int32: data },
    }
}
fn string_option(
    value: ESteamNetworkingConfigValue,
    data: &CString,
) -> SteamNetworkingConfigValue_t {
    SteamNetworkingConfigValue_t {
        m_eValue: value,
        m_eDataType: ESteamNetworkingConfigDataType::k_ESteamNetworkingConfig_String,
        m_val: SteamNetworkingConfigValue_t__bindgen_ty_1 {
            m_string: data.as_ptr(),
        },
    }
}
fn turn_strings(servers: &[TurnServer]) -> Result<(CString, CString, CString), TransportError> {
    if servers.len() > 4
        || servers.iter().any(|s| {
            [s.address.as_str(), s.username.as_str(), s.password.as_str()]
                .iter()
                .any(|v| {
                    v.is_empty()
                        || v.len() > 256
                        || v.bytes().any(|b| !b.is_ascii_graphic() || b == b',')
                })
        })
    {
        return Err(TransportError::ProtocolViolation);
    }
    let join = |f: fn(&TurnServer) -> &str| {
        CString::new(servers.iter().map(f).collect::<Vec<_>>().join(","))
            .map_err(|_| TransportError::ProtocolViolation)
    };
    Ok((
        join(|s| &s.address)?,
        join(|s| &s.username)?,
        join(|s| &s.password)?,
    ))
}
struct Options {
    stun: CString,
    turn: CString,
    users: CString,
    passwords: CString,
    public: bool,
    #[cfg(test)]
    relay_only: bool,
}
impl Options {
    fn new(config: &IceConfig) -> Result<Self, TransportError> {
        Ok(Self {
            stun: CString::new(config.stun_servers.join(","))
                .map_err(|_| TransportError::ProtocolViolation)?,
            turn: CString::default(),
            users: CString::default(),
            passwords: CString::default(),
            public: config.allow_public_candidates,
            #[cfg(test)]
            relay_only: std::env::var_os("JIGSALL_TURN_TEST_ONLY").is_some(),
        })
    }
    fn values(&self) -> [SteamNetworkingConfigValue_t; 7] {
        use ESteamNetworkingConfigValue::*;
        #[cfg(test)]
        let private = if self.relay_only {
            0
        } else {
            k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Private
        };
        #[cfg(not(test))]
        let private = k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Private;
        [
            int_option(
                k_ESteamNetworkingConfig_P2P_Transport_ICE_Enable,
                private
                    | if !self.turn.as_bytes().is_empty() {
                        k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Relay
                    } else {
                        0
                    }
                    | if self.public {
                        k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Public
                    } else {
                        0
                    },
            ),
            int_option(k_ESteamNetworkingConfig_P2P_Transport_ICE_Implementation, 1),
            string_option(k_ESteamNetworkingConfig_P2P_STUN_ServerList, &self.stun),
            string_option(k_ESteamNetworkingConfig_P2P_TURN_ServerList, &self.turn),
            string_option(k_ESteamNetworkingConfig_P2P_TURN_UserList, &self.users),
            string_option(k_ESteamNetworkingConfig_P2P_TURN_PassList, &self.passwords),
            int_option(
                k_ESteamNetworkingConfig_TimeoutInitial,
                crate::network::lifecycle::CONNECTING_TIMEOUT.as_millis() as i32,
            ),
        ]
    }
}

pub(super) struct Listener {
    handle: HSteamListenSocket,
    options: Options,
    port: u16,
}
/// The synchronous receive callback spends credit only for a new, validated
/// request, never for duplicate/continuation signaling packets.
pub(super) struct IncomingAdmission<'a> {
    pub allow: bool,
    pub origin: Option<Origin>,
    pub pending: usize,
    pub gate: &'a mut Admission,
    pub starts: &'a mut Bucket,
    pub now: Instant,
}
impl IncomingAdmission<'_> {
    fn accept(&mut self) -> bool {
        self.allow
            && self.starts.available(1, self.now)
            && self.gate.admit(self.origin, self.pending, self.now).is_ok()
            && self.starts.take(1, self.now)
    }
}
impl Listener {
    pub(super) fn new(port: u16, config: &IceConfig) -> Result<Self, TransportError> {
        global()?;
        let options = Options::new(config)?;
        let values = options.values();
        // SAFETY: initialized options/string storage lives through the call;
        // GNS copies config values. This owner closes the returned listener.
        let handle = unsafe {
            SteamAPI_ISteamNetworkingSockets_CreateListenSocketP2P(
                interface(),
                port.into(),
                values.len() as i32,
                values.as_ptr(),
            )
        };
        if handle == 0 {
            return Err(failure());
        }
        Ok(Self {
            handle,
            options,
            port,
        })
    }
    pub(super) fn install_turn(&mut self, servers: &[TurnServer]) -> Result<(), TransportError> {
        let strings = turn_strings(servers)?;
        self.options.turn = strings.0;
        self.options.users = strings.1;
        self.options.passwords = strings.2;
        self.apply_options()
    }
    pub(super) fn disable_turn(&mut self) -> Result<(), TransportError> {
        self.options.turn = CString::default();
        self.options.users = CString::default();
        self.options.passwords = CString::default();
        // values() removes only the Relay bit, preserving private/public/STUN settings.
        self.apply_options()
    }
    fn apply_options(&self) -> Result<(), TransportError> {
        // GNS copies these into the listener; future incoming connections inherit them.
        for value in self.options.values() {
            let data = if value.m_eDataType
                == ESteamNetworkingConfigDataType::k_ESteamNetworkingConfig_String
            {
                unsafe { value.m_val.m_string.cast::<c_void>() }
            } else {
                std::ptr::addr_of!(value.m_val).cast::<c_void>()
            };
            if !unsafe {
                SteamAPI_ISteamNetworkingUtils_SetConfigValue(
                    SteamAPI_SteamNetworkingUtils_v003(),
                    value.m_eValue,
                    ESteamNetworkingConfigScope::k_ESteamNetworkingConfig_ListenSocket,
                    self.handle as isize,
                    value.m_eDataType,
                    data,
                )
            } {
                return Err(failure());
            }
        }
        Ok(())
    }
    pub(super) fn connect(
        &self,
        peer: PeerId,
        port: u16,
        mailbox: SignalingEndpoint,
    ) -> Result<Connection, TransportError> {
        let values = self.options.values();
        let identity = identity(peer);
        let signal = signaling(peer, mailbox);
        // SAFETY: GNS takes ownership of signal on success AND failure; identity
        // and option pointers are read/copied before this call returns.
        let handle = unsafe {
            SteamAPI_ISteamNetworkingSockets_ConnectP2PCustomSignaling(
                interface(),
                signal,
                &identity,
                port.into(),
                values.len() as i32,
                values.as_ptr(),
            )
        };
        if handle == 0 {
            return Err(failure());
        }
        let connection = Connection(Some(handle));
        connection.configure()?;
        Ok(connection)
    }
    pub(super) fn receive(
        &self,
        from: PeerId,
        bytes: &[u8],
        admission: IncomingAdmission<'_>,
        mailbox: SignalingEndpoint,
    ) -> Option<Connection> {
        let mut ctx = ReceiveContext {
            from,
            port: self.port,
            admission,
            mailbox,
            incoming: None,
        };
        if bytes.is_empty() || bytes.len() > MAX_SIGNAL_BYTES {
            return None;
        }
        // SAFETY: GNS explicitly does not retain the receive context or payload.
        // on_request runs synchronously on this thread; only it mutates ctx.
        // Outgoing adapters have their own separately boxed SendContext.
        unsafe {
            SteamAPI_ISteamNetworkingSockets_ReceivedP2PCustomSignal2(
                interface(),
                bytes.as_ptr().cast(),
                bytes.len() as i32,
                (&mut ctx as *mut ReceiveContext).cast(),
                Some(on_request),
                None,
            );
        }
        ctx.incoming
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        // SAFETY: unique listener owner, after the backend drops its connections.
        unsafe {
            SteamAPI_ISteamNetworkingSockets_CloseListenSocket(interface(), self.handle);
        }
    }
}
struct ReceiveContext<'a> {
    from: PeerId,
    port: u16,
    admission: IncomingAdmission<'a>,
    mailbox: SignalingEndpoint,
    incoming: Option<Connection>,
}
unsafe extern "C" fn on_request(
    ctx: *mut c_void,
    handle: HSteamNetConnection,
    identity: *const SteamNetworkingIdentity,
    port: i32,
) -> *mut ISteamNetworkingConnectionSignaling {
    // SAFETY: stack context and native identity are valid only during receive.
    // No panicking user code, mutex unwraps, or gameplay callbacks cross FFI.
    let ctx = unsafe { &mut *ctx.cast::<ReceiveContext<'_>>() };
    if ctx.incoming.is_some()
        || port != i32::from(ctx.port)
        || identity.is_null()
        || peer(unsafe { &*identity }) != Some(ctx.from)
        || !ctx.admission.accept()
    {
        return ptr::null_mut();
    }
    let connection = Connection(Some(handle));
    if connection.configure().is_err()
        || check(unsafe { SteamAPI_ISteamNetworkingSockets_AcceptConnection(interface(), handle) })
            .is_err()
    {
        return ptr::null_mut();
    }
    let signal = signaling(ctx.from, ctx.mailbox.clone());
    ctx.incoming = Some(connection);
    signal
}

pub(super) enum State {
    Pending,
    Connected,
    Closed,
    Failed,
}
/// Unique native connection owner. Handles never leave this module.
pub(super) struct Connection(Option<HSteamNetConnection>);
impl Connection {
    fn handle(&self) -> HSteamNetConnection {
        self.0.unwrap_or(0)
    }
    pub(super) fn configure_authenticated_send_rate(&self) -> Result<(), TransportError> {
        super::super::configure_authenticated_send_rate(::gns::GnsConnection::from_raw(
            self.handle(),
        ))
    }
    fn configure(&self) -> Result<(), TransportError> {
        // SAFETY: live handle and fixed initialized lane arrays, copied by GNS.
        check(unsafe {
            SteamAPI_ISteamNetworkingSockets_ConfigureConnectionLanes(
                interface(),
                self.handle(),
                3,
                [0, 0, 1].as_ptr(),
                [1u16, 4, 1].as_ptr(),
            )
        })
    }
    pub(super) fn state(&self) -> State {
        let mut info = SteamNetConnectionInfo_t::default();
        // SAFETY: valid writable output, native connection remains owned here.
        if !unsafe {
            SteamAPI_ISteamNetworkingSockets_GetConnectionInfo(
                interface(),
                self.handle(),
                &mut info,
            )
        } {
            return State::Failed;
        }
        use ESteamNetworkingConnectionState::*;
        match info.m_eState {
            k_ESteamNetworkingConnectionState_Connected => State::Connected,
            k_ESteamNetworkingConnectionState_Connecting
            | k_ESteamNetworkingConnectionState_FindingRoute => State::Pending,
            k_ESteamNetworkingConnectionState_ClosedByPeer => State::Closed,
            _ => State::Failed,
        }
    }
    pub(super) fn egress(&self) -> Result<(u64, u64), TransportError> {
        let mut status = SteamNetConnectionRealTimeStatus_t::default();
        let mut lanes = [SteamNetConnectionRealTimeLaneStatus_t::default(); 3];
        // SAFETY: output buffers have exactly the requested native size/count.
        check(unsafe {
            SteamAPI_ISteamNetworkingSockets_GetConnectionRealTimeStatus(
                interface(),
                self.handle(),
                &mut status,
                3,
                lanes.as_mut_ptr(),
            )
        })?;
        Ok((
            status.m_cbPendingReliable as u64 + status.m_cbSentUnackedReliable as u64,
            lanes[2].m_cbPendingReliable as u64 + lanes[2].m_cbSentUnackedReliable as u64,
        ))
    }
    pub(super) fn send(
        &self,
        lane: u16,
        class: MessageClass,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        if bytes.len() > k_cbMaxSteamNetworkingSocketsMessageSizeSend as usize {
            return Err(TransportError::PayloadTooLarge);
        }
        // SAFETY: allocate a GNS-owned initialized buffer of the checked length.
        let message = unsafe {
            SteamAPI_ISteamNetworkingUtils_AllocateMessage(
                SteamAPI_SteamNetworkingUtils_v003(),
                bytes.len() as i32,
            )
        };
        if message.is_null() {
            return Err(failure());
        }
        // SAFETY: unique allocation, writable payload with exactly bytes.len()
        // capacity. SendMessages(deleteFailed=true) releases on every outcome,
        // so ownership is transferred once, without a Rust destructor afterward.
        unsafe {
            if !bytes.is_empty() {
                ptr::copy_nonoverlapping(bytes.as_ptr(), (*message).m_pData.cast(), bytes.len());
            }
            (*message).m_conn = self.handle();
            (*message).m_idxLane = lane;
            (*message).m_nFlags = if class == MessageClass::Transient {
                k_nSteamNetworkingSend_Unreliable
                    | k_nSteamNetworkingSend_NoNagle
                    | k_nSteamNetworkingSend_NoDelay
            } else {
                k_nSteamNetworkingSend_Reliable
            };
            let mut messages = [message];
            let mut result = 0;
            SteamAPI_ISteamNetworkingSockets_SendMessages(
                interface(),
                1,
                messages.as_mut_ptr(),
                &mut result,
                true,
            );
            if result < 0 {
                return Err(failure());
            }
        }
        Ok(())
    }
    pub(super) fn receive(&self) -> Result<Option<Message>, TransportError> {
        let mut message = ptr::null_mut();
        // SAFETY: one output slot; successful receive transfers one reference
        // to Message's RAII guard, released even on validation failures.
        match unsafe {
            SteamAPI_ISteamNetworkingSockets_ReceiveMessagesOnConnection(
                interface(),
                self.handle(),
                &mut message,
                1,
            )
        } {
            0 => Ok(None),
            1 if !message.is_null() => Ok(Some(Message(message))),
            _ => Err(failure()),
        }
    }
    pub(super) fn close(&mut self, code: i32) {
        if let Some(handle) = self.0.take() {
            // SAFETY: consume the unique owned handle exactly once; no linger.
            unsafe {
                SteamAPI_ISteamNetworkingSockets_CloseConnection(
                    interface(),
                    handle,
                    code,
                    ptr::null(),
                    false,
                );
            }
        }
    }
    #[cfg(test)]
    pub(super) fn assert_send_rate(&self, expected: i32) {
        for option in [
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMin,
            ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_SendRateMax,
        ] {
            assert_eq!(
                global()
                    .unwrap()
                    .utils()
                    .get_connection_config_value(
                        ::gns::GnsConnection::from_raw(self.handle()),
                        option
                    )
                    .unwrap(),
                ::gns::GnsConfigValue::Int32(expected),
            );
        }
    }
    #[cfg(test)]
    pub(super) fn assert_turn_user(&self, expected: &str) {
        let value = global()
            .unwrap()
            .utils()
            .get_connection_config_value(
                ::gns::GnsConnection::from_raw(self.handle()),
                ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_TURN_UserList,
            )
            .unwrap();
        assert!(value == ::gns::GnsConfigValue::String(expected.to_owned()));
        if expected.is_empty() {
            for option in [
                ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_TURN_ServerList,
                ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_TURN_PassList,
            ] {
                let value = global()
                    .unwrap()
                    .utils()
                    .get_connection_config_value(
                        ::gns::GnsConnection::from_raw(self.handle()),
                        option,
                    )
                    .unwrap();
                assert!(value == ::gns::GnsConfigValue::String(String::new()));
            }
            let ::gns::GnsConfigValue::Int32(enabled) = global()
                .unwrap()
                .utils()
                .get_connection_config_value(
                    ::gns::GnsConnection::from_raw(self.handle()),
                    ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_Transport_ICE_Enable,
                )
                .unwrap()
            else {
                panic!("expected ICE flags")
            };
            assert_eq!(
                enabled & k_nSteamNetworkingConfig_P2P_Transport_ICE_Enable_Relay,
                0
            );
        }
    }
    #[cfg(test)]
    pub(super) fn is_relay(&self) -> bool {
        let mut info = SteamNetConnectionInfo_t::default();
        assert!(unsafe {
            SteamAPI_ISteamNetworkingSockets_GetConnectionInfo(
                interface(),
                self.handle(),
                &mut info,
            )
        });
        info.m_nFlags & k_nSteamNetworkConnectionInfoFlags_Relayed != 0
    }
    #[cfg(test)]
    pub(super) fn details(&self) -> String {
        let mut info = SteamNetConnectionInfo_t::default();
        // SAFETY: correctly sized initialized output, owned live connection.
        assert!(unsafe {
            SteamAPI_ISteamNetworkingSockets_GetConnectionInfo(
                interface(),
                self.handle(),
                &mut info,
            )
        });
        assert_eq!(
            info.m_nFlags & k_nSteamNetworkConnectionInfoFlags_LoopbackBuffers,
            0
        );
        if std::env::var_os("JIGSALL_TURN_TEST_ONLY").is_some() {
            assert_ne!(
                info.m_nFlags & k_nSteamNetworkConnectionInfoFlags_Relayed,
                0,
                "TURN route required"
            );
        } else {
            assert_eq!(
                info.m_nFlags & k_nSteamNetworkConnectionInfoFlags_Relayed,
                0
            );
        }
        let remote_port = info.m_addrRemote.m_port;
        if info.m_nFlags & k_nSteamNetworkConnectionInfoFlags_Relayed == 0 {
            assert_ne!(remote_port, 0, "ICE must select an actual UDP route");
        }
        let bytes: Vec<_> = info
            .m_szConnectionDescription
            .iter()
            .copied()
            .take_while(|c| *c != 0)
            .map(|c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.close(1000);
    }
}
pub(super) struct Message(*mut SteamNetworkingMessage_t);
impl Message {
    pub(super) fn lane(&self) -> u16 {
        // SAFETY: live native message reference, uniquely owned by this guard.
        unsafe { (*self.0).m_idxLane }
    }
    pub(super) fn payload(&self) -> Result<&[u8], TransportError> {
        // SAFETY: live native message; validate size before constructing slice.
        let message = unsafe { &*self.0 };
        if message.m_cbSize < 0 || message.m_cbSize > k_cbMaxSteamNetworkingSocketsMessageSizeSend {
            return Err(failure());
        }
        if message.m_cbSize == 0 {
            return Ok(&[]);
        }
        if message.m_pData.is_null() {
            return Err(failure());
        }
        Ok(
            unsafe {
                std::slice::from_raw_parts(message.m_pData.cast(), message.m_cbSize as usize)
            },
        )
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        // SAFETY: exactly one received reference; payload cannot outlive &self.
        unsafe {
            SteamAPI_SteamNetworkingMessage_t_Release(self.0);
        }
    }
}

use super::{
    protocol::{self, ClientMessage, Frame, ServerMessage, MAX_WS_BYTES},
    EndpointUrl, RendezvousError, CHANNEL_CAPACITY,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    Connector,
};

pub(super) struct Worker {
    commands: mpsc::Sender<ClientMessage>,
    events: mpsc::Receiver<ServerMessage>,
    stop: Option<oneshot::Sender<()>>,
    done: Arc<AtomicBool>,
    terminal: Arc<Mutex<Option<RendezvousError>>>,
}
impl Worker {
    pub fn start(endpoint: EndpointUrl) -> Result<Self, RendezvousError> {
        let (commands, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (tx, events) = mpsc::channel(CHANNEL_CAPACITY);
        let (stop, stopped) = oneshot::channel();
        let done = Arc::new(AtomicBool::new(false));
        let terminal = Arc::new(Mutex::new(None));
        let worker_done = done.clone();
        let worker_terminal = terminal.clone();
        std::thread::Builder::new()
            .name("rendezvous-ws".into())
            .spawn(move || {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => {
                        let result = runtime.block_on(async {
                            tokio::select! {
                                biased;
                                _ = stopped => Err(RendezvousError::Requested),
                                result = run(endpoint, rx, tx) => result,
                            }
                        });
                        runtime.shutdown_timeout(Duration::from_secs(1));
                        result
                    }
                    Err(_) => Err(RendezvousError::Network),
                };
                *worker_terminal.lock().unwrap() =
                    Some(result.err().unwrap_or(RendezvousError::Network));
                worker_done.store(true, Ordering::Release);
            })
            .map_err(|_| RendezvousError::Network)?;
        Ok(Self {
            commands,
            events,
            stop: Some(stop),
            done,
            terminal,
        })
    }
    pub fn send(&self, message: ClientMessage) -> Result<(), RendezvousError> {
        self.send_owned(message).map_err(|(e, _)| e)
    }
    pub fn send_owned(
        &self,
        message: ClientMessage,
    ) -> Result<(), (RendezvousError, ClientMessage)> {
        self.commands.try_send(message).map_err(|e| match e {
            mpsc::error::TrySendError::Full(message) => (RendezvousError::Backpressure, message),
            mpsc::error::TrySendError::Closed(message) => (RendezvousError::Network, message),
        })
    }
    pub fn pop(&mut self) -> Option<ServerMessage> {
        self.events.try_recv().ok()
    }
    pub fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
    pub fn finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
    pub fn terminal(&self) -> Option<RendezvousError> {
        if self.events.is_empty() {
            self.terminal.lock().unwrap().take()
        } else {
            None
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
async fn run(
    endpoint: EndpointUrl,
    mut commands: mpsc::Receiver<ClientMessage>,
    events: mpsc::Sender<ServerMessage>,
) -> Result<(), RendezvousError> {
    let config = WebSocketConfig::default()
        .read_buffer_size(MAX_WS_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_WS_BYTES * 2)
        .max_message_size(Some(MAX_WS_BYTES))
        .max_frame_size(Some(MAX_WS_BYTES));
    let connector = Connector::Rustls(Arc::new(tls_config()?));
    let (socket, _) = timeout(
        Duration::from_secs(10),
        connect_async_tls_with_config(&endpoint.0, Some(config), true, Some(connector)),
    )
    .await
    .map_err(|_| RendezvousError::Timeout)?
    .map_err(|_| RendezvousError::Network)?;
    let (mut sink, mut stream) = socket.split();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_ping = Instant::now() + Duration::from_secs(15);
    let mut outstanding: Option<([u8; 8], Instant)> = None;
    let mut sequence = 0u64;
    let mut window = Instant::now();
    let mut frames = 0usize;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = Instant::now();
                if outstanding.is_some_and(|(_, deadline)| now >= deadline) { return Err(RendezvousError::Timeout); }
                if outstanding.is_none() && now >= next_ping {
                    sequence += 1;
                    let nonce = sequence.to_be_bytes();
                    outstanding = Some((nonce, now + Duration::from_secs(15)));
                    timeout(Duration::from_secs(2), sink.send(Message::Ping(nonce.to_vec().into()))).await.map_err(|_| RendezvousError::Timeout)?.map_err(|_| RendezvousError::Network)?;
                }
            }
            message = commands.recv() => {
                let Some(message) = message else { return Err(RendezvousError::Requested); };
                let json = serde_json::to_string(&Frame::new(message)).map_err(|_| RendezvousError::ProtocolViolation)?;
                if json.len() > MAX_WS_BYTES { return Err(RendezvousError::ProtocolViolation); }
                timeout(Duration::from_secs(2), sink.send(Message::Text(json.into()))).await.map_err(|_| RendezvousError::Timeout)?.map_err(|_| RendezvousError::Network)?;
            }
            message = stream.next() => {
                let now = Instant::now();
                if now.saturating_duration_since(window) >= Duration::from_secs(1) { window = now; frames = 0; }
                frames += 1;
                if frames > 512 { return Err(RendezvousError::Backpressure); }
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let message = protocol::parse_server(&text).map_err(|_| RendezvousError::ProtocolViolation)?;
                        events.try_send(message).map_err(|_| RendezvousError::Backpressure)?;
                    }
                    Some(Ok(Message::Ping(_))) => {
                        timeout(Duration::from_secs(2), sink.flush()).await.map_err(|_| RendezvousError::Timeout)?.map_err(|_| RendezvousError::Network)?;
                    }
                    Some(Ok(Message::Pong(bytes))) => {
                        if outstanding.is_some_and(|(nonce, _)| bytes.as_ref() == nonce) {
                            outstanding = None;
                            next_ping = now + Duration::from_secs(15);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return Err(RendezvousError::Network),
                    _ => return Err(RendezvousError::ProtocolViolation),
                }
            }
        }
    }
}

pub(super) fn tls_config() -> Result<rustls::ClientConfig, RendezvousError> {
    // Select the provider explicitly, without changing process-global defaults
    // or depending on some unrelated crate enabling a rustls provider feature.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| RendezvousError::Network)?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

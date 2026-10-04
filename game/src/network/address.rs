//! Parse join endpoints without DNS on the frame thread; resolve names in a worker.
use crossbeam::channel::{bounded, Receiver, TryRecvError};
use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESOLVERS: usize = 4;
static ACTIVE_RESOLVERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAddress {
    host: String,
    port: u16,
    literal: Option<IpAddr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidAddress;

fn usable(address: SocketAddr) -> bool {
    let ip = address.ip().to_canonical();
    address.port() != 0 && !ip.is_unspecified() && !ip.is_multicast()
}

impl FromStr for ServerAddress {
    type Err = InvalidAddress;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if let Ok(address) = value.parse::<SocketAddr>() {
            if !usable(address) {
                return Err(InvalidAddress);
            }
            return Ok(Self {
                host: address.ip().to_string(),
                port: address.port(),
                literal: Some(address.ip()),
            });
        }
        let (host, port) = value.rsplit_once(':').ok_or(InvalidAddress)?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidAddress);
        }
        let port: u16 = port.parse().map_err(|_| InvalidAddress)?;
        let name = host.strip_suffix('.').unwrap_or(host);
        if port == 0
            || name.is_empty()
            || name.len() > 253
            || name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            || !name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(InvalidAddress);
        }
        Ok(Self {
            host: host.to_owned(),
            port,
            literal: None,
        })
    }
}

impl ServerAddress {
    /// Literal IPs bypass DNS entirely.
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.literal.map(|ip| SocketAddr::new(ip, self.port))
    }

    pub fn resolve(self) -> Result<AddressResolution, ResolutionError> {
        if let Some(address) = self.socket_addr() {
            let (sender, receiver) = bounded(1);
            // A fresh channel always has room for its single reply.
            let _ = sender.send(Ok(address));
            return Ok(AddressResolution {
                receiver: Some(receiver),
                deadline: Instant::now() + RESOLUTION_TIMEOUT,
            });
        }
        AddressResolution::spawn(move || {
            let addresses = (self.host.as_str(), self.port)
                .to_socket_addrs()
                .map_err(|error| ResolutionError::Lookup(error.to_string()))?;
            choose_address(addresses)
        })
    }
}

fn choose_address(
    addresses: impl IntoIterator<Item = SocketAddr>,
) -> Result<SocketAddr, ResolutionError> {
    // The default listener is IPv4. Keep resolver order within each family and
    // support IPv6-only names, rather than choosing an unusable ::1 for localhost.
    let mut ipv6 = None;
    for address in addresses.into_iter().filter(|address| usable(*address)) {
        if address.ip().to_canonical().is_ipv4() {
            return Ok(address);
        }
        ipv6.get_or_insert(address);
    }
    ipv6.ok_or(ResolutionError::NoUsableAddress)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionError {
    Lookup(String),
    NoUsableAddress,
    Timeout,
    Busy,
}

/// Dropping this request discards late replies. Workers own no World or secrets.
pub struct AddressResolution {
    receiver: Option<Receiver<Result<SocketAddr, ResolutionError>>>,
    deadline: Instant,
}

struct ResolverSlot;
impl Drop for ResolverSlot {
    fn drop(&mut self) {
        ACTIVE_RESOLVERS.fetch_sub(1, Ordering::Relaxed);
    }
}

impl AddressResolution {
    fn spawn(
        resolve: impl FnOnce() -> Result<SocketAddr, ResolutionError> + Send + 'static,
    ) -> Result<Self, ResolutionError> {
        // An OS lookup cannot be interrupted. Bound workers even if requests
        // are repeatedly cancelled or time out while the OS is still resolving.
        ACTIVE_RESOLVERS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < MAX_RESOLVERS).then_some(active + 1)
            })
            .map_err(|_| ResolutionError::Busy)?;
        let slot = ResolverSlot;
        let (sender, receiver) = bounded(1);
        let deadline = Instant::now() + RESOLUTION_TIMEOUT;
        std::thread::Builder::new()
            .name("jigsall-dns".into())
            .spawn(move || {
                let _slot = slot;
                let _ = sender.send(resolve());
            })
            .map_err(|error| ResolutionError::Lookup(error.to_string()))?;
        Ok(Self {
            receiver: Some(receiver),
            deadline,
        })
    }

    /// Called once per frame. A completed or timed-out request cannot reply again.
    pub fn poll(&mut self, now: Instant) -> Option<Result<SocketAddr, ResolutionError>> {
        let receiver = self.receiver.as_ref()?;
        let result = if now >= self.deadline {
            Err(ResolutionError::Timeout)
        } else {
            match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => Err(ResolutionError::Lookup(
                    "resolver stopped without a reply".into(),
                )),
            }
        };
        self.receiver = None;
        Some(result)
    }
}

#[cfg(test)]
mod tests;

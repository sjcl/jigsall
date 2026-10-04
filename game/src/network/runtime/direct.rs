//! Direct-IP establishment and explicit listener lifetime, outside common Runtime.
use super::*;

pub(super) struct DirectIpDriver<T> {
    pub runtime: Runtime<T>,
    listener: Option<ListenerId>,
}
impl<T> std::ops::Deref for DirectIpDriver<T> {
    type Target = Runtime<T>;
    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}
impl<T> std::ops::DerefMut for DirectIpDriver<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}
impl<T: DirectIpTransport> DirectIpDriver<T> {
    #[cfg(test)]
    pub fn host(
        backend: T,
        options: HostOptions,
        definition: PuzzleDefinition,
        image: Option<Arc<[u8]>>,
        now: Instant,
    ) -> Result<Self, RuntimeStartError> {
        let prepared = PreparedHost::new(definition, image, options.session)?;
        Self::host_request(backend, &mut Some(options), prepared, now)
    }
    pub fn host_request(
        mut backend: T,
        request: &mut Option<HostOptions>,
        prepared: PreparedHost,
        now: Instant,
    ) -> Result<Self, RuntimeStartError> {
        let options = request.as_ref().ok_or(RuntimeStartError::AlreadyActive)?;
        let listener = backend
            .listen(options.address)
            .map_err(RuntimeStartError::Transport)?;
        let address = match backend.listener_address(listener) {
            Ok(address) => address,
            Err(error) => {
                let _ = backend.close_listener(listener);
                return Err(RuntimeStartError::Transport(error));
            }
        };
        let options = request.take().unwrap();
        let mut runtime = Runtime::host_with_transport(
            backend,
            HostRuntimeOptions {
                display_name: options.display_name,
                session: options.session,
                host: options.host,
                password: options.password,
            },
            prepared,
            now,
        );
        runtime.status.address = Some(address);
        Ok(Self {
            runtime,
            listener: Some(listener),
        })
    }
    pub fn client(
        mut backend: T,
        options: JoinOptions,
        limits: ImageDecodeLimits,
    ) -> Result<Self, RuntimeStartError> {
        let connection = backend
            .connect(options.address)
            .map_err(RuntimeStartError::Transport)?;
        let mut runtime = Runtime::client_with_connection(
            backend,
            connection,
            ClientRuntimeOptions {
                display_name: options.display_name,
                password: options.password,
                cached_image: options.cached_image,
            },
            limits,
        );
        runtime.status.address = Some(options.address);
        Ok(Self {
            runtime,
            listener: None,
        })
    }
    fn close_listener(&mut self) {
        if let Some(listener) = self.listener.take() {
            // Use SecureTransport's drain/barrier semantics, even on failure.
            let _ = self.runtime.transport.close_listener(listener);
        }
    }
    #[cfg(test)]
    pub fn teardown(&mut self, store: &mut PieceDataStore) {
        self.runtime.teardown(store);
        self.close_listener();
    }
}
impl<T: DirectIpTransport + 'static> world::RuntimeDriver for DirectIpDriver<T> {
    fn poll(&mut self, world: &mut World) {
        world::RuntimeDriver::poll(&mut self.runtime, world);
    }
    fn commands(&mut self, world: &mut World, commands: Vec<ClientCommand>) {
        world::RuntimeDriver::commands(&mut self.runtime, world, commands);
    }
    fn teardown(&mut self, world: &mut World) {
        world::RuntimeDriver::teardown(&mut self.runtime, world);
        self.close_listener();
    }
    fn active(&self) -> bool {
        self.runtime.active
    }
    fn authority(&self) -> Option<&AuthoritySession> {
        self.runtime.session.as_ref()
    }
    fn replica(&self) -> Option<&PeerReplicationState> {
        world::RuntimeDriver::replica(&self.runtime)
    }
    #[cfg(test)]
    fn host_state(&self) -> Option<&HostState> {
        world::RuntimeDriver::host_state(&self.runtime)
    }
}

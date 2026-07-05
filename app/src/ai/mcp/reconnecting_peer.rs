//! A peer wrapper that transparently handles reconnection when the underlying transport is closed.

use std::future::Future;

use uuid::Uuid;
use warpui::ModelSpawner;

use super::TemplatableMCPServerManager;

/// A wrapper around an MCP server connection that transparently handles reconnection.
///
/// When making requests (e.g., `call_tool` or `read_resource`), this type checks if the
/// underlying transport is closed and automatically triggers reconnection before retrying
/// the request.
#[derive(Clone)]
pub struct ReconnectingPeer {
    installation_uuid: Uuid,
    spawner: ModelSpawner<TemplatableMCPServerManager>,
}

/// Error type for reconnecting peer operations.
#[derive(Debug, thiserror::Error)]
pub enum ReconnectingPeerError {
    #[error("Service error: {0}")]
    Service(#[from] rmcp::ServiceError),
    #[error("Reconnection failed: {0}")]
    ReconnectionFailed(String),
    #[error("Model dropped")]
    ModelDropped,
}

impl From<ReconnectingPeerError> for rmcp::ServiceError {
    fn from(e: ReconnectingPeerError) -> Self {
        rmcp::ServiceError::McpError(rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::INTERNAL_ERROR,
            message: e.to_string().into(),
            data: None,
        })
    }
}

impl ReconnectingPeer {
    /// Creates a new `ReconnectingPeer` with the given installation UUID and spawner.
    pub fn new(
        installation_uuid: Uuid,
        spawner: ModelSpawner<TemplatableMCPServerManager>,
    ) -> Self {
        Self {
            installation_uuid,
            spawner,
        }
    }

    /// Gets the current peer if connected, or triggers reconnection and waits for it.
    async fn get_connected_peer(
        &self,
    ) -> Result<rmcp::Peer<rmcp::RoleClient>, ReconnectingPeerError> {
        let installation_uuid = self.installation_uuid;

        // First, check if we have a connected peer.
        let peer_result = self
            .spawner
            .spawn(move |manager, _ctx| manager.get_peer_if_connected(installation_uuid))
            .await
            .map_err(|_| ReconnectingPeerError::ModelDropped)?;

        if let Some(peer) = peer_result {
            return Ok(peer);
        }

        // Peer is not connected, trigger reconnection.
        self.reconnect_peer().await
    }

    /// Forces reconnection and waits for the new peer.
    async fn reconnect_peer(&self) -> Result<rmcp::Peer<rmcp::RoleClient>, ReconnectingPeerError> {
        let installation_uuid = self.installation_uuid;
        log::debug!("Triggering reconnection for MCP server {installation_uuid}");
        let (tx, rx) = tokio::sync::oneshot::channel();

        self.spawner
            .spawn(move |manager, ctx| {
                manager.reconnect_server(installation_uuid, tx, ctx);
            })
            .await
            .map_err(|_| ReconnectingPeerError::ModelDropped)?;

        // Wait for reconnection to complete.
        let peer = rx
            .await
            .map_err(|_| ReconnectingPeerError::ReconnectionFailed("Channel closed".to_string()))?
            .map_err(|e| ReconnectingPeerError::ReconnectionFailed(e.to_string()))?;

        log::debug!("Reconnection completed for MCP server {installation_uuid}");
        Ok(peer)
    }

    /// Executes a request with automatic retry on dead-transport errors.
    ///
    /// If the initial request fails because the transport is closed or cannot accept the
    /// request, the reconnecting peer will force a reconnect before retrying.
    ///
    /// Note: We intentionally retry only once to avoid infinite reconnection loops if the
    /// server is persistently failing. If the retry also fails, the error propagates to the
    /// caller.
    async fn with_reconnect_retry<T, R, F, Fut>(
        &self,
        params: T,
        f: F,
    ) -> Result<R, rmcp::ServiceError>
    where
        T: Clone,
        F: Fn(rmcp::Peer<rmcp::RoleClient>, T) -> Fut,
        Fut: Future<Output = Result<R, rmcp::ServiceError>>,
    {
        let get_connected_peer = {
            let reconnecting_peer = self.clone();
            move || {
                let reconnecting_peer = reconnecting_peer.clone();
                async move {
                    reconnecting_peer
                        .get_connected_peer()
                        .await
                        .map_err(rmcp::ServiceError::from)
                }
            }
        };
        let reconnect_peer = {
            let reconnecting_peer = self.clone();
            move || {
                let reconnecting_peer = reconnecting_peer.clone();
                async move {
                    reconnecting_peer
                        .reconnect_peer()
                        .await
                        .map_err(rmcp::ServiceError::from)
                }
            }
        };
        with_reconnect_retry_from_peer_source(params, get_connected_peer, reconnect_peer, f).await
    }

    /// Calls a tool on the MCP server.
    pub async fn call_tool(
        &self,
        params: rmcp::model::CallToolRequestParam,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ServiceError> {
        self.with_reconnect_retry(params, |peer, p| async move { peer.call_tool(p).await })
            .await
    }

    /// Reads a resource from the MCP server.
    pub async fn read_resource(
        &self,
        params: rmcp::model::ReadResourceRequestParam,
    ) -> Result<rmcp::model::ReadResourceResult, rmcp::ServiceError> {
        self.with_reconnect_retry(params, |peer, p| async move { peer.read_resource(p).await })
            .await
    }
}

fn should_retry_after_service_error(error: &rmcp::ServiceError) -> bool {
    matches!(
        error,
        rmcp::ServiceError::TransportClosed | rmcp::ServiceError::TransportSend(_)
    )
}

async fn with_reconnect_retry_from_peer_source<
    P,
    T,
    R,
    GetPeer,
    GetPeerFuture,
    ReconnectPeer,
    ReconnectPeerFuture,
    F,
    Fut,
>(
    params: T,
    get_connected_peer: GetPeer,
    reconnect_peer: ReconnectPeer,
    f: F,
) -> Result<R, rmcp::ServiceError>
where
    T: Clone,
    GetPeer: Fn() -> GetPeerFuture,
    GetPeerFuture: Future<Output = Result<P, rmcp::ServiceError>>,
    ReconnectPeer: Fn() -> ReconnectPeerFuture,
    ReconnectPeerFuture: Future<Output = Result<P, rmcp::ServiceError>>,
    F: Fn(P, T) -> Fut,
    Fut: Future<Output = Result<R, rmcp::ServiceError>>,
{
    let peer = get_connected_peer().await?;
    match f(peer, params.clone()).await {
        Err(error) if should_retry_after_service_error(&error) => {
            let peer = reconnect_peer().await?;
            f(peer, params).await
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct FakePeer {
        generation: u8,
    }

    #[derive(Debug, Default)]
    struct RetryProbe {
        get_connected_peer_count: usize,
        reconnect_peer_count: usize,
        call_attempts: Vec<(u8, String)>,
    }

    impl RetryProbe {
        fn get_connected_peer(&mut self) -> FakePeer {
            self.get_connected_peer_count += 1;
            FakePeer { generation: 1 }
        }

        fn reconnect_peer(&mut self) -> FakePeer {
            self.reconnect_peer_count += 1;
            FakePeer { generation: 2 }
        }

        fn record_call(&mut self, peer: FakePeer, params: String) -> usize {
            self.call_attempts.push((peer.generation, params));
            self.call_attempts.len()
        }
    }

    fn transport_send_error() -> rmcp::ServiceError {
        rmcp::ServiceError::TransportSend(rmcp::transport::DynamicTransportError {
            transport_name: Cow::Borrowed("test"),
            transport_type_id: std::any::TypeId::of::<()>(),
            error: Box::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "send failed",
            )),
        })
    }

    #[test]
    fn retry_policy_only_retries_dead_transport() {
        assert!(should_retry_after_service_error(
            &rmcp::ServiceError::TransportClosed
        ));
        assert!(should_retry_after_service_error(&transport_send_error()));

        let server_error = rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
            rmcp::model::ErrorCode::INTERNAL_ERROR,
            "server exploded",
            None,
        ));
        assert!(!should_retry_after_service_error(&server_error));
        assert!(!should_retry_after_service_error(
            &rmcp::ServiceError::UnexpectedResponse
        ));
        assert!(!should_retry_after_service_error(
            &rmcp::ServiceError::Cancelled {
                reason: Some("user cancelled".to_owned())
            }
        ));
        assert!(!should_retry_after_service_error(
            &rmcp::ServiceError::Timeout {
                timeout: Duration::from_secs(3)
            }
        ));
    }

    #[tokio::test]
    async fn reconnect_retry_reconnects_before_second_attempt_after_dead_transport() {
        let probe = Arc::new(Mutex::new(RetryProbe::default()));

        let result = with_reconnect_retry_from_peer_source(
            "payload".to_owned(),
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().get_connected_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().reconnect_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move |peer, params| {
                    let probe = probe.clone();
                    async move {
                        let attempt = probe.lock().unwrap().record_call(peer, params);
                        if attempt == 1 {
                            Err(rmcp::ServiceError::TransportClosed)
                        } else {
                            Ok(peer.generation)
                        }
                    }
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(result, 2);
        let probe = probe.lock().unwrap();
        assert_eq!(probe.get_connected_peer_count, 1);
        assert_eq!(probe.reconnect_peer_count, 1);
        assert_eq!(
            probe.call_attempts,
            vec![(1, "payload".to_owned()), (2, "payload".to_owned())]
        );
    }

    #[tokio::test]
    async fn reconnect_retry_retries_dead_transport_only_once() {
        let probe = Arc::new(Mutex::new(RetryProbe::default()));

        let result = with_reconnect_retry_from_peer_source(
            "payload".to_owned(),
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().get_connected_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().reconnect_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move |peer, params| {
                    let probe = probe.clone();
                    async move {
                        probe.lock().unwrap().record_call(peer, params);
                        Err::<(), _>(rmcp::ServiceError::TransportClosed)
                    }
                }
            },
        )
        .await;

        assert!(matches!(result, Err(rmcp::ServiceError::TransportClosed)));
        let probe = probe.lock().unwrap();
        assert_eq!(probe.get_connected_peer_count, 1);
        assert_eq!(probe.reconnect_peer_count, 1);
        assert_eq!(
            probe.call_attempts,
            vec![(1, "payload".to_owned()), (2, "payload".to_owned())]
        );
    }

    #[tokio::test]
    async fn reconnect_retry_does_not_retry_mcp_server_error() {
        let probe = Arc::new(Mutex::new(RetryProbe::default()));
        let server_error = || {
            rmcp::ServiceError::McpError(rmcp::model::ErrorData::new(
                rmcp::model::ErrorCode::INTERNAL_ERROR,
                "server exploded",
                None,
            ))
        };

        let result = with_reconnect_retry_from_peer_source(
            "payload".to_owned(),
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().get_connected_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().reconnect_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move |peer, params| {
                    let probe = probe.clone();
                    async move {
                        probe.lock().unwrap().record_call(peer, params);
                        Err::<(), _>(server_error())
                    }
                }
            },
        )
        .await;

        assert!(matches!(result, Err(rmcp::ServiceError::McpError(_))));
        let probe = probe.lock().unwrap();
        assert_eq!(probe.get_connected_peer_count, 1);
        assert_eq!(probe.reconnect_peer_count, 0);
        assert_eq!(probe.call_attempts, vec![(1, "payload".to_owned())]);
    }

    #[tokio::test]
    async fn reconnect_retry_does_not_retry_tool_returned_error_result() {
        let probe = Arc::new(Mutex::new(RetryProbe::default()));

        let result = with_reconnect_retry_from_peer_source(
            "payload".to_owned(),
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().get_connected_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move || {
                    let probe = probe.clone();
                    async move { Ok(probe.lock().unwrap().reconnect_peer()) }
                }
            },
            {
                let probe = probe.clone();
                move |peer, params| {
                    let probe = probe.clone();
                    async move {
                        probe.lock().unwrap().record_call(peer, params);
                        Ok(rmcp::model::CallToolResult::error(vec![
                            rmcp::model::Content::text("tool rejected args"),
                        ]))
                    }
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(result.is_error, Some(true));
        let probe = probe.lock().unwrap();
        assert_eq!(probe.get_connected_peer_count, 1);
        assert_eq!(probe.reconnect_peer_count, 0);
        assert_eq!(probe.call_attempts, vec![(1, "payload".to_owned())]);
    }
}

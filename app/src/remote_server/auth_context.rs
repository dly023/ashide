use std::sync::Arc;

use remote_server::auth::RemoteServerAuthContext;

use crate::auth::AuthState;

/// 构造 Environment Runtime transport 使用的 identity context。
pub fn environment_runtime_auth_context(auth_state: Arc<AuthState>) -> RemoteServerAuthContext {
    RemoteServerAuthContext::new(move || remote_server_identity_key(&auth_state))
}

fn remote_server_identity_key(auth_state: &AuthState) -> String {
    auth_state
        .user_id()
        .map(|uid| uid.as_string())
        .unwrap_or_else(|| auth_state.local_identity_key())
}

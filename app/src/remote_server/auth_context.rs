use std::sync::Arc;

use remote_server::auth::RemoteServerAuthContext;
use warpui::r#async::BoxFuture;

use crate::auth::AuthState;

/// 构造给 remote-server 模块使用的 auth context。
///
/// Remote runtime 使用本地 auth context。Bearer token 来源直接读取
/// `AuthState::get_access_token_ignoring_validity()`(仅在用户挂了 BYOP API key 时返回
/// `Some`,其余永远 `None`)。
pub fn server_api_auth_context(auth_state: Arc<AuthState>) -> RemoteServerAuthContext {
    let token_auth_state = auth_state.clone();
    let identity_auth_state = auth_state;

    RemoteServerAuthContext::new(
        move || -> BoxFuture<'static, Option<String>> {
            let token = token_auth_state.get_access_token_ignoring_validity();
            Box::pin(async move { token })
        },
        move || remote_server_identity_key(&identity_auth_state),
    )
}

fn remote_server_identity_key(auth_state: &AuthState) -> String {
    // Ashide 统一用本地 `user_id()`；没有用户时才用本地身份兜底 key。
    auth_state
        .user_id()
        .map(|uid| uid.as_string())
        .unwrap_or_else(|| auth_state.local_identity_key())
}

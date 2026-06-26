//! Ashide 本地身份模块。
//!
//! Ashide 是纯本地工具,启动后直接进入本地用户态;本模块只负责给需要
//! 身份字段的模型提供稳定的本地用户、BYOP API key 读取入口和 onboarding 状态。
//! 它不创建账号会话、不刷新远端 token,也不发起登录/登出网络请求。

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity};

use crate::server_time::ServerTimestamp;

pub const TEST_USER_EMAIL: &str = "test_user@ashide.local";
pub const TEST_USER_UID: &str = "test_user_uid";

pub mod user_uid;

pub use user_uid::UserUid;

#[derive(Clone, Copy, Debug)]
pub enum OwnerType {
    Team,
    User,
}

/// Ashide BYOP API key 前缀。
///
/// 用户输入裸 key 时会统一补成此前缀,便于所有 provider 设置读取同一种格式。
pub const API_KEY_PREFIX: &str = "wk-";

// ---------- Credentials / AuthToken / LoginToken ----------
//
// Ashide 只区分用户自带 BYOP API key 和测试态本地身份。

/// 表示用户与 Ashide 的本地身份凭据。
///
/// - `ApiKey`:BYOP 路径下用户自带 LLM provider API key。
/// - `Test`:测试构建下的本地身份。
#[derive(Clone, Debug)]
pub enum Credentials {
    /// BYOP API key。Ashide 不把 key 绑定到组织 owner。
    ApiKey {
        key: String,
        owner_type: Option<OwnerType>,
    },
    /// 测试构建下的本地身份。
    Test,
}

impl Credentials {
    /// 返回 API key 字符串(仅当 variant 为 [`Credentials::ApiKey`])。
    pub fn as_api_key(&self) -> Option<&str> {
        match self {
            Credentials::ApiKey { key, .. } => Some(key),
            Credentials::Test => None,
        }
    }

    /// 返回 API key owner type(Ashide 路径下永远 `None`)。
    pub fn api_key_owner_type(&self) -> Option<OwnerType> {
        match self {
            Credentials::ApiKey { owner_type, .. } => *owner_type,
            Credentials::Test => None,
        }
    }

    /// 返回要写入 Authorization 头的 bearer token。
    ///
    /// 本地化后只有 `ApiKey` 产出真实值;`Test` 返回 [`AuthToken::NoAuth`]。
    pub fn bearer_token(&self) -> AuthToken {
        match self {
            Credentials::ApiKey { key, .. } => AuthToken::ApiKey(key.clone()),
            Credentials::Test => AuthToken::NoAuth,
        }
    }
}

/// HTTP 请求头使用的短期 token。
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// BYOP / 平台层 API key。
    ApiKey(String),
    /// 无任何 token(session cookie / test / Ashide 本地模式)。
    NoAuth,
}

impl AuthToken {
    /// 返回 bearer token 字符串(若有)。
    pub fn bearer_token(&self) -> Option<String> {
        match self {
            AuthToken::ApiKey(key) => Some(key.clone()),
            AuthToken::NoAuth => None,
        }
    }

    /// 返回 Authorization 头使用的 token 引用。
    pub fn as_bearer_token(&self) -> Option<&str> {
        match self {
            AuthToken::ApiKey(key) => Some(key),
            AuthToken::NoAuth => None,
        }
    }
}

// ---------- User 元数据 ----------

/// 匿名用户类型。Ashide 本地身份不会构造 `Some(AnonymousUserType::...)`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnonymousUserType {
    NativeClientAnonymousUser,
    NativeClientAnonymousUserFeatureGated,
    WebClientAnonymousUser,
}

/// 认证 principal 类型。Ashide 本地身份等同 `User`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalType {
    #[default]
    User,
    ServiceAccount,
}

/// 用户元数据,只保留界面展示需要的字段。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserMetadata {
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
}

/// 当前本地用户。
#[derive(Debug, Clone)]
pub struct User {
    pub local_id: UserUid,
    pub metadata: UserMetadata,
    pub is_onboarded: bool,
    pub needs_sso_link: bool,
    pub anonymous_user_type: Option<AnonymousUserType>,
    pub is_on_work_domain: bool,
    pub linked_at: Option<ServerTimestamp>,
    pub principal_type: PrincipalType,
}

impl User {
    /// 用于显示的用户名 — display_name 优先,否则 email。
    pub fn username_for_display(&self) -> &str {
        self.metadata
            .display_name
            .as_deref()
            .unwrap_or(self.metadata.email.as_str())
    }

    /// 用户显示名,不回退到 email。
    pub fn display_name(&self) -> Option<String> {
        self.metadata.display_name.clone()
    }

    /// 默认本地用户。Ashide 在所有本地身份路径下都使用此用户。
    pub fn test() -> Self {
        Self {
            local_id: UserUid::new(TEST_USER_UID),
            metadata: UserMetadata {
                email: TEST_USER_EMAIL.to_string(),
                display_name: None,
                photo_url: None,
            },
            is_onboarded: true,
            needs_sso_link: false,
            anonymous_user_type: None,
            is_on_work_domain: false,
            linked_at: None,
            principal_type: PrincipalType::User,
        }
    }

    /// 用户是否匿名。Ashide 永远返回 `false`。
    pub fn is_user_anonymous(&self) -> bool {
        false
    }

    pub fn anonymous_user_type(&self) -> Option<AnonymousUserType> {
        self.anonymous_user_type
    }

    pub fn linked_at(&self) -> Option<ServerTimestamp> {
        self.linked_at
    }
}

// ---------- AuthState ----------

/// 当前本地身份状态。
///
/// Ashide 不接账号系统,因此登录态、匿名态、重新认证需求都由本地身份
/// 规则直接给出;`user_id()` 返回稳定的本地 [`UserUid`]。
pub struct AuthState {
    user: RwLock<Option<User>>,
    credentials: RwLock<Option<Credentials>>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new_for_test()
    }
}

impl AuthState {
    /// 创建本地默认 AuthState。
    pub fn new() -> Self {
        Self {
            user: RwLock::new(Some(User::test())),
            credentials: RwLock::new(Some(Credentials::Test)),
        }
    }

    /// 测试场景下构造 AuthState(等价于 [`AuthState::new`])。
    pub fn new_for_test() -> Self {
        Self::new()
    }

    /// 初始化 AuthState。`api_key` 参数被忠实保留(BYOP 入口仍可能传入),
    /// 本地身份不做额外账号检查。
    #[cfg_attr(target_family = "wasm", allow(unused_variables))]
    pub fn initialize(_ctx: &AppContext, api_key: Option<String>) -> Self {
        let state = Self::new();
        if let Some(api_key_value) = api_key {
            let formatted = if api_key_value.starts_with(API_KEY_PREFIX) {
                api_key_value
            } else {
                format!("{API_KEY_PREFIX}{api_key_value}")
            };
            *state.credentials.write() = Some(Credentials::ApiKey {
                key: formatted,
                owner_type: None,
            });
        }
        state
    }

    /// 用户是否已登录。Ashide 本地身份启动即为已登录。
    pub fn is_logged_in(&self) -> bool {
        true
    }

    /// 是否匿名或登出。Ashide 本地身份不进入匿名/登出态。
    pub fn is_anonymous_or_logged_out(&self) -> bool {
        false
    }

    /// 返回缓存的 access token(忽略有效性)。Ashide 路径下仅当用户挂了
    /// `Credentials::ApiKey` 才有值。
    pub fn get_access_token_ignoring_validity(&self) -> Option<String> {
        self.credentials
            .read()
            .as_ref()?
            .bearer_token()
            .bearer_token()
    }

    pub fn username_for_display(&self) -> Option<String> {
        Some(self.user.read().as_ref()?.username_for_display().to_owned())
    }

    pub fn display_name(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .and_then(|user| user.display_name())
    }

    pub fn user_email(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .map(|user| user.metadata.email.clone())
    }

    pub fn is_onboarded(&self) -> Option<bool> {
        self.user.read().as_ref().map(|user| user.is_onboarded)
    }

    pub fn user_email_domain(&self) -> Option<String> {
        self.user.read().as_ref().map(|user| {
            user.metadata
                .email
                .split('@')
                .nth(1)
                .unwrap_or("")
                .to_string()
        })
    }

    pub fn is_user_anonymous(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_user_web_anonymous_user(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_anonymous_user_feature_gated(&self) -> Option<bool> {
        Some(false)
    }

    pub fn user_photo_url(&self) -> Option<String> {
        self.user
            .read()
            .as_ref()
            .and_then(|user| user.metadata.photo_url.clone())
    }

    pub fn needs_sso_link(&self) -> Option<bool> {
        Some(false)
    }

    pub fn anonymous_user_type(&self) -> Option<AnonymousUserType> {
        None
    }

    /// 标记用户为已 onboarded。
    pub fn set_is_onboarded(&self, is_onboarded: bool) {
        if let Some(user) = self.user.write().as_mut() {
            user.is_onboarded = is_onboarded;
        }
    }

    pub fn user_id(&self) -> Option<UserUid> {
        self.user.read().as_ref().map(|user| user.local_id)
    }

    /// 返回本地身份分桶 key。
    pub fn local_identity_key(&self) -> String {
        Uuid::nil().to_string()
    }

    /// 返回是否需要重新认证。Ashide 本地身份不需要重新认证。
    pub fn needs_reauth(&self) -> bool {
        false
    }

    /// 返回当前用户的 anonymous renotification block 是否过期。Ashide 用户
    /// 不被视作匿名用户,该函数返回 `false`(永不弹注册提示)。
    pub fn anonymous_user_renotification_block_expired(
        &self,
        _last_time_opt: Option<String>,
    ) -> bool {
        false
    }

    pub fn is_on_work_domain(&self) -> Option<bool> {
        Some(false)
    }

    pub fn is_api_key_authenticated(&self) -> bool {
        matches!(
            self.credentials.read().as_ref(),
            Some(Credentials::ApiKey { .. })
        )
    }

    pub fn api_key(&self) -> Option<String> {
        self.credentials
            .read()
            .as_ref()
            .and_then(|c| c.as_api_key().map(|s| s.to_owned()))
    }

    pub fn principal_type(&self) -> Option<PrincipalType> {
        Some(PrincipalType::User)
    }

    pub fn is_service_account(&self) -> bool {
        false
    }

    pub fn api_key_owner_type(&self) -> Option<OwnerType> {
        self.credentials.read().as_ref()?.api_key_owner_type()
    }

    /// 返回当前 credentials 的克隆。
    pub fn credentials(&self) -> Option<Credentials> {
        self.credentials.read().clone()
    }

    /// 将本地 auth 状态恢复到本地占位用户的默认快照，用于 `log_out` 及本地重置路径。
    pub fn reset_local_defaults(&self) {
        *self.user.write() = Some(User::test());
        *self.credentials.write() = Some(Credentials::Test);
    }
}

impl warp_managed_secrets::ActorProvider for AuthState {
    fn actor_uid(&self) -> Option<String> {
        self.user_id().map(|uid| uid.as_string())
    }
}

/// AuthState 的 singleton 包装。
pub struct AuthStateProvider {
    auth_state: Arc<AuthState>,
}

impl AuthStateProvider {
    pub fn new(auth_state: Arc<AuthState>) -> Self {
        Self { auth_state }
    }

    pub fn new_for_test() -> Self {
        Self {
            auth_state: Arc::new(AuthState::new_for_test()),
        }
    }

    /// 构造测试用 AuthState provider。
    ///
    /// Ashide 本地身份没有登出态;需要覆盖“非 API key 用户”路径的测试使用该构造器。
    pub fn new_logged_out_for_test() -> Self {
        Self::new_for_test()
    }

    pub fn get(&self) -> &Arc<AuthState> {
        &self.auth_state
    }
}

impl Entity for AuthStateProvider {
    type Event = ();
}

impl SingletonEntity for AuthStateProvider {}

// ---------- AuthManager ----------

/// 本地身份门控功能标识。
pub type LoginGatedFeature = &'static str;

/// 本地身份门控视图变体。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthViewVariant {
    Initial,
    RequireLoginCloseable,
    ShareRequirementCloseable,
}

// ---------- 本地身份门控 view ----------
//
// 本地身份没有登录表单。这些 view 只承载少量既有窗口状态事件,不渲染账号 UI。
//
// 运行时这些 view 代码路径仍会被创建但不渲染(`View::render` 返回 `Empty`)、
// 事件不被触发(原 UI 交互点已不存在)。

use warpui::elements::Empty;
use warpui::{Element, View, ViewContext};

/// 本地身份门控 view。
pub struct AuthView {
    variant: AuthViewVariant,
}

impl AuthView {
    pub fn new(variant: AuthViewVariant, _ctx: &mut ViewContext<Self>) -> Self {
        Self { variant }
    }

    pub fn set_variant(&mut self, _ctx: &mut ViewContext<Self>, variant: AuthViewVariant) {
        self.variant = variant;
    }

    /// 返回当前 variant。
    pub fn variant(&self) -> AuthViewVariant {
        self.variant
    }

    /// 本地身份没有浏览器登录步骤。
    pub fn skip_to_browser_open_step(&mut self, _ctx: &mut ViewContext<Self>) {}
}

impl Entity for AuthView {
    type Event = AuthViewEvent;
}

impl View for AuthView {
    fn ui_name() -> &'static str {
        "AuthView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for AuthView {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

#[derive(Debug)]
pub enum AuthViewEvent {
    Close,
}

/// 本地身份覆盖提醒 modal。
pub struct AuthOverrideWarningModal;

impl AuthOverrideWarningModal {
    pub fn new(_ctx: &mut ViewContext<Self>, _variant: AuthOverrideWarningModalVariant) -> Self {
        Self
    }
}

impl Entity for AuthOverrideWarningModal {
    type Event = AuthOverrideWarningModalEvent;
}

impl View for AuthOverrideWarningModal {
    fn ui_name() -> &'static str {
        "AuthOverrideWarningModal"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for AuthOverrideWarningModal {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

#[derive(Debug)]
pub enum AuthOverrideWarningModalEvent {
    Close,
    BulkExport,
}

#[derive(Clone, Copy, Debug)]
pub enum AuthOverrideWarningModalVariant {
    OnboardingView,
    WorkspaceModal,
}

/// SSO 链接状态 view。Ashide 本地身份不会展示该 view。
pub struct NeedsSsoLinkView;

impl NeedsSsoLinkView {
    pub fn new() -> Self {
        Self
    }

    pub fn set_email(&mut self, _email: String) {}
}

impl Default for NeedsSsoLinkView {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for NeedsSsoLinkView {
    type Event = ();
}

impl View for NeedsSsoLinkView {
    fn ui_name() -> &'static str {
        "NeedsSsoLinkView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

impl warpui::TypedActionView for NeedsSsoLinkView {
    type Action = ();
    fn handle_action(&mut self, _action: &(), _ctx: &mut ViewContext<Self>) {}
}

/// Web host handoff view。Ashide native 不使用该 view。
pub struct WebHandoffView;

impl WebHandoffView {
    pub fn new(_ctx: &mut ViewContext<Self>) -> Self {
        Self
    }
}

impl Entity for WebHandoffView {
    type Event = WebHandoffEvent;
}

impl View for WebHandoffView {
    fn ui_name() -> &'static str {
        "WebHandoffView"
    }

    fn render(&self, _app: &AppContext) -> Box<dyn Element> {
        Box::new(Empty::new())
    }
}

#[derive(Debug)]
pub enum WebHandoffEvent {
    Unsupported,
}

/// AuthManager 事件。
#[derive(Debug)]
pub enum AuthManagerEvent {
    AuthComplete,
    AuthFailed(UserAuthenticationError),
    SkippedLogin,
    NeedsReauth,
    AttemptedLoginGatedFeature {
        auth_view_variant: AuthViewVariant,
    },
    /// 低频 失败:同上。
    CreateAnonymousUserFailed,
}

/// 用户认证错误。Ashide native 本地身份正常路径不会触发这些错误。
#[derive(Debug, thiserror::Error)]
pub enum UserAuthenticationError {
    #[error("Access token denied")]
    DeniedAccessToken,
    #[error("User account disabled")]
    UserAccountDisabled,
    #[error("Invalid state parameter")]
    InvalidStateParameter,
    #[error("Missing state parameter")]
    MissingStateParameter,
    #[error("Unexpected error: {0}")]
    Unexpected(anyhow::Error),
}

/// 持久化在 SQLite `current_user_information` 表里的当前用户信息。
/// `persistence/sqlite.rs` 与 `persistence/mod.rs` 仍消费该 struct,保留。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedCurrentUserInformation {
    pub email: String,
}

/// 本地身份管理器。
///
/// 负责本地用户快照、onboarding 标记和身份重置。
pub struct AuthManager {
    auth_state: Arc<AuthState>,
}

impl AuthManager {
    /// 创建 AuthManager。
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let auth_state = AuthStateProvider::as_ref(ctx).get().clone();
        Self { auth_state }
    }

    /// 测试场景构造,与 [`Self::new`] 等价。
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        Self::new(ctx)
    }

    /// 刷新当前用户态。
    ///
    /// 本地身份在启动时已经可用,刷新不会发起网络请求。
    pub fn refresh_user(&self, _ctx: &mut ModelContext<Self>) {}

    /// 重置本地身份快照。
    ///
    /// Ashide 本地身份没有远端登出流程;该入口只恢复默认本地用户。
    pub(crate) fn log_out(&mut self, _ctx: &mut ModelContext<Self>) {
        self.auth_state.reset_local_defaults();
        log::debug!("AuthManager::log_out 已本地 reset: 已切换为本地占位用户态");
    }

    /// 标记需要重新认证。Ashide 本地身份忽略该状态。
    pub fn set_needs_reauth(&mut self, _new_value: bool, _ctx: &mut ModelContext<Self>) {}

    /// 创建本地用户并发出 `AuthComplete` 让 onboarding 流推进。
    pub fn create_anonymous_user(
        &mut self,
        _referral_code: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(AuthManagerEvent::AuthComplete);
    }

    /// 本地身份下登录门控不展示账号 UI。
    pub fn attempt_login_gated_feature(
        &mut self,
        _feature: LoginGatedFeature,
        _auth_view_variant: AuthViewVariant,
        _ctx: &mut ModelContext<Self>,
    ) {
    }

    /// 用户引导走完后置本地 onboarded 标记。
    pub fn set_user_onboarded(&mut self, ctx: &mut ModelContext<Self>) {
        self.auth_state.set_is_onboarded(true);
        ctx.emit(AuthManagerEvent::AuthComplete);
    }
}

impl Entity for AuthManager {
    type Event = AuthManagerEvent;
}

impl SingletonEntity for AuthManager {}

// ---------- 全模块 init ----------

/// Ashide 本地身份模块 init。
pub fn init(_app: &mut AppContext) {}

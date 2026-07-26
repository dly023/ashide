use crate::channel::ChannelState;

// 上游 Warp 的文档站/Slack/隐私政策对 Ashide 不再适用，
// 未接入 Ashide 自有渠道前，这些入口保持空串。
// `ctx.open_url("")` 在 UI 调用方是无害 no-op。
pub const USER_DOCS_URL: &str = "";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const GITHUB_ISSUES_URL: &str = "https://github.com/dly023/ashide/issues";
pub const SLACK_URL: &str = "";
pub const PRIVACY_POLICY_URL: &str = "";

pub fn feedback_form_url() -> String {
    let mut url = url::Url::parse("https://github.com/dly023/ashide/issues/new/choose")
        .expect("Should not fail to parse");
    if let Some(version) = ChannelState::app_version() {
        url.query_pairs_mut().append_pair("ashide-version", version);
    }
    url.query_pairs_mut()
        .append_pair("os-version", &os_info::get().version().to_string());
    url.to_string()
}

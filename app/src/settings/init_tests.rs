use settings_value::SettingsValue;

use crate::terminal::session_settings::{NotificationsMode, NotificationsSettings};

// NotificationsSettings 的 serde 格式与文件(SettingsValue)格式存在差异:
//   - NotificationsMode: serde 用 PascalCase("Enabled"),文件用 snake_case("enabled")
//   - Duration: serde 用 {"secs":N,"nanos":N},文件用纯整数
// 这两个用例确保 from_file_value 在遇到 serde 格式时返回 None,而不是因
// #[serde(default)] 静默回落到默认值。

#[test]
fn test_notifications_from_file_value_rejects_serde_format_enum() {
    // serde serializes NotificationsMode::Enabled as "Enabled" (PascalCase),
    // but from_file_value expects "enabled" (snake_case). When the field is
    // present but unparsable, from_file_value should return None — not
    // silently fall back to the #[serde(default)] value (Unset).
    let serde_json_value = serde_json::to_value(NotificationsSettings {
        mode: NotificationsMode::Enabled,
        ..NotificationsSettings::default()
    })
    .unwrap();

    let result = NotificationsSettings::from_file_value(&serde_json_value);
    assert!(
        result.is_none(),
        "from_file_value should reject serde-format enum values, but got: {result:?}"
    );
}

#[test]
fn test_notifications_from_file_value_rejects_serde_format_duration() {
    // serde serializes Duration as {"secs": N, "nanos": N}, but
    // Duration::from_file_value expects a plain integer. Use file-format
    // for mode ("unset") so that the failure is isolated to the Duration field.
    let json = serde_json::json!({
        "mode": "unset",
        "is_long_running_enabled": true,
        "long_running_threshold": {"secs": 60, "nanos": 0},
        "is_password_prompt_enabled": true,
        "is_agent_task_completed_enabled": true,
        "is_needs_attention_enabled": true,
        "play_notification_sound": true,
    });

    let result = NotificationsSettings::from_file_value(&json);
    assert!(
        result.is_none(),
        "from_file_value should reject serde-format Duration, but got: {result:?}"
    );
}

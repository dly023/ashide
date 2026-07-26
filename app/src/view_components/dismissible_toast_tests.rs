use super::*;

fn transient_policy(source: &str) -> ToastPolicy {
    ToastPolicy::transient(source)
}

#[test]
fn same_key_replaces_in_place_and_keeps_one_carrier() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    let (uuid, generation) = stack.upsert(
        DismissibleToast::error("first".to_owned()),
        transient_policy("terminal"),
    );
    let (replacement_uuid, replacement_generation) = stack.upsert(
        DismissibleToast::error("second".to_owned()),
        transient_policy("terminal"),
    );

    assert_eq!(stack.toasts.len(), 1);
    assert_eq!(replacement_uuid, uuid);
    assert_eq!(generation + 1, replacement_generation);
    assert_eq!(stack.toasts[0].dismissible_toast.main_text, "second");
}

#[test]
fn replacement_resets_timer_generation_and_stale_timer_is_ignored() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    let (uuid, stale_generation) = stack.upsert(
        DismissibleToast::error("first".to_owned()),
        transient_policy("terminal"),
    );
    let (_, current_generation) = stack.upsert(
        DismissibleToast::error("second".to_owned()),
        transient_policy("terminal"),
    );

    assert!(!stack.remove_toast_generation(&uuid, stale_generation));
    assert_eq!(stack.toasts.len(), 1);
    assert!(stack.remove_toast_generation(&uuid, current_generation));
    assert!(stack.toasts.is_empty());
}

#[test]
fn unkeyed_error_burst_is_bounded_by_source_and_category() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    for index in 0..100 {
        stack.upsert(
            DismissibleToast::error(format!("error {index}")),
            ToastPolicy::transient("terminal"),
        );
    }

    assert_eq!(stack.toasts.len(), 1);
    assert_eq!(stack.toasts[0].dismissible_toast.main_text, "error 99");
}

#[test]
fn action_required_stays_and_is_dismissable() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    let (uuid, generation) = stack.upsert(
        DismissibleToast::error("Fix configuration".to_owned()),
        ToastPolicy::action_required("settings", "invalid-config"),
    );

    assert_eq!(stack.toasts[0].lifetime, ToastLifetime::ActionRequired);
    assert!(!stack.remove_toast_generation(&uuid, generation + 1));
    assert!(stack.remove_toast_generation(&uuid, generation));
    assert!(stack.toasts.is_empty());
}

#[test]
fn different_sources_remain_independent() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    for source in ["terminal", "session-navigator"] {
        stack.upsert(
            DismissibleToast::error(source.to_owned()),
            ToastPolicy::transient(source),
        );
    }

    assert_eq!(stack.toasts.len(), 2);
}

#[test]
fn visible_projection_is_bounded_and_summarizes_overflow() {
    let mut stack = DismissibleToastStack::<()>::new(Duration::from_secs(4));
    for index in 0..10 {
        stack.upsert(
            DismissibleToast::success(format!("success {index}")),
            ToastPolicy::action_required("task", index.to_string()),
        );
    }

    assert_eq!(stack.visible_toast_count(), MAX_VISIBLE_TOASTS - 1);
    assert_eq!(stack.hidden_toast_count(), 6);
    assert_eq!(stack.visible_toast_count() + 1, MAX_VISIBLE_TOASTS);
}

#[test]
fn action_required_policy_always_has_a_stable_key() {
    let policy = ToastPolicy::action_required("settings", "invalid-config");
    let (identity, lifetime) = policy.resolve(ToastCategory::Error);

    assert_eq!(identity.stable_key.as_deref(), Some("invalid-config"));
    assert_eq!(lifetime, ToastLifetime::ActionRequired);
}

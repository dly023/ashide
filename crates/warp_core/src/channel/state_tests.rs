use crate::{channel::Channel, AppId};

use super::{channel_from_app_id, logfile_name_from_channel, ChannelState};

/// `ChannelState::init()` (the static default for OSS builds) must satisfy
/// the cloud-disabled predicate; the cloud-removal plan's Phase 5 short-circuit
/// depends on this invariant.
#[test]
fn default_oss_state_is_cloud_disabled() {
    assert!(ChannelState::is_cloud_disabled());
}

#[test]
fn app_variant_channels_select_runtime_log_file() {
    assert_eq!(logfile_name_from_channel(Channel::Oss), "ashide.log");
    assert_eq!(logfile_name_from_channel(Channel::Stable), "ashide.log");
    assert_eq!(logfile_name_from_channel(Channel::Dev), "ashide_dev.log");
    assert_eq!(
        logfile_name_from_channel(Channel::Local),
        "ashide_local.log"
    );
    assert_eq!(
        logfile_name_from_channel(Channel::Preview),
        "ashide_preview.log"
    );
    assert_eq!(
        logfile_name_from_channel(Channel::Integration),
        "ashide_integration.log"
    );
}

#[test]
fn app_variant_bundle_ids_select_runtime_channel() {
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "Ashide")),
        Some(Channel::Oss)
    );
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "AshideDev")),
        Some(Channel::Dev)
    );
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "AshideLocal")),
        Some(Channel::Local)
    );
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "AshidePreview")),
        Some(Channel::Preview)
    );
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "AshideIntegration")),
        Some(Channel::Integration)
    );
    assert_eq!(
        channel_from_app_id(&AppId::new("dev", "ashide", "AshideLegacy")),
        None
    );
}

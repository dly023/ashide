use warpui::{Entity, ModelContext, SingletonEntity, WindowId};

use crate::{
    view_components::{DismissibleToast, ToastPolicy, ToastType},
    workspace::WorkspaceAction,
};

/// A global model that provides an interface to open a workspace-level
/// toast. This allows callers to add a toast from any context that has
/// access to the AppContext.
#[derive(Copy, Clone, Debug)]
pub struct ToastStack;

impl From<ToastType> for DismissibleToast<WorkspaceAction> {
    fn from(value: ToastType) -> Self {
        match value {
            ToastType::StoredObjectNotFound => {
                DismissibleToast::error(crate::t!("common-resource-not-found-or-access-denied"))
            }
        }
    }
}

impl ToastStack {
    #[track_caller]
    pub fn add_transient_toast(
        &mut self,
        toast: DismissibleToast<WorkspaceAction>,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(ToastStackEvent::AddToast {
            window_id,
            toast,
            policy: ToastPolicy::transient(std::panic::Location::caller().file()),
        });
    }

    pub fn add_action_required_toast(
        &mut self,
        toast: DismissibleToast<WorkspaceAction>,
        source: impl Into<String>,
        stable_key: impl Into<String>,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(ToastStackEvent::AddToast {
            window_id,
            toast,
            policy: ToastPolicy::action_required(source, stable_key),
        });
    }

    #[track_caller]
    pub fn add_transient_toast_by_type(
        &mut self,
        toast_type: ToastType,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.add_transient_toast(toast_type.into(), window_id, ctx);
    }

    pub fn remove_toast_by_identifier(
        &mut self,
        identifier: String,
        window_id: WindowId,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(ToastStackEvent::RemoveToast {
            window_id,
            identifier,
        });
    }
}

#[allow(clippy::enum_variant_names)]
pub enum ToastStackEvent {
    AddToast {
        /// The window for which this event is for.
        window_id: WindowId,
        toast: DismissibleToast<WorkspaceAction>,
        policy: ToastPolicy,
    },
    RemoveToast {
        /// The window for which this event is for.
        window_id: WindowId,
        identifier: String,
    },
}

impl Entity for ToastStack {
    type Event = ToastStackEvent;
}

impl SingletonEntity for ToastStack {}

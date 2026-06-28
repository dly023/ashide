use warp_core::ui::appearance::Appearance;
use warpui::{
    elements::{Align, Container, CrossAxisAlignment, Flex, ParentElement, Text},
    AppContext, Element, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WindowId,
};

use crate::{
    app_state::{EnvironmentLifecycleState, LeafContents},
    pane_group::{
        focus_state::PaneFocusHandle,
        pane::{view, ShareableLink, ShareableLinkError},
        BackingView, PaneConfiguration, PaneContent, PaneEvent, PaneGroup, PaneView,
    },
    workspace::WorkspaceRegistry,
};

use super::{DetachType, PaneId};

pub struct EnvironmentRuntimePlaceholderPane {
    view: ViewHandle<PaneView<EnvironmentRuntimePlaceholderView>>,
    pane_configuration: ModelHandle<PaneConfiguration>,
}

impl EnvironmentRuntimePlaceholderPane {
    pub fn new<V: View>(ctx: &mut ViewContext<V>) -> Self {
        let window_id = ctx.window_id();
        let placeholder_view = ctx.add_typed_action_view(move |ctx| {
            EnvironmentRuntimePlaceholderView::new(window_id, ctx)
        });
        let pane_configuration = placeholder_view.as_ref(ctx).pane_configuration();
        let placeholder_view_for_pane = placeholder_view.clone();
        let pane_view = ctx.add_typed_action_view(|ctx| {
            let pane_id = PaneId::from_environment_runtime_placeholder_pane_ctx(ctx);
            PaneView::new(
                pane_id,
                placeholder_view_for_pane,
                (),
                pane_configuration.clone(),
                ctx,
            )
        });
        let pane_id = PaneId::from_environment_runtime_placeholder_pane_view(&pane_view);
        placeholder_view.update(ctx, |view, _| view.set_pane_id(pane_id));

        Self {
            view: pane_view,
            pane_configuration,
        }
    }
}

impl PaneContent for EnvironmentRuntimePlaceholderPane {
    fn id(&self) -> PaneId {
        PaneId::from_environment_runtime_placeholder_pane_view(&self.view)
    }

    fn attach(
        &self,
        _group: &PaneGroup,
        focus_handle: PaneFocusHandle,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        self.view
            .update(ctx, |view, ctx| view.set_focus_handle(focus_handle, ctx));

        let pane_id = self.id();
        let child = self.view.as_ref(ctx).child(ctx);
        ctx.subscribe_to_view(&child, move |pane_group, _, event, ctx| {
            pane_group.handle_pane_event(pane_id, event, ctx);
        });
    }

    fn detach(
        &self,
        _group: &PaneGroup,
        _detach_type: DetachType,
        ctx: &mut ViewContext<PaneGroup>,
    ) {
        let child = self.view.as_ref(ctx).child(ctx);
        ctx.unsubscribe_to_view(&child);
    }

    fn snapshot(&self, _app: &AppContext) -> LeafContents {
        LeafContents::EnvironmentRuntimePlaceholder
    }

    fn has_application_focus(&self, ctx: &mut ViewContext<PaneGroup>) -> bool {
        self.view.is_self_or_child_focused(ctx)
    }

    fn focus(&self, ctx: &mut ViewContext<PaneGroup>) {
        self.view
            .as_ref(ctx)
            .child(ctx)
            .update(ctx, BackingView::focus_contents)
    }

    fn shareable_link(
        &self,
        _ctx: &mut ViewContext<PaneGroup>,
    ) -> Result<ShareableLink, ShareableLinkError> {
        Ok(ShareableLink::Base)
    }

    fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    fn is_pane_being_dragged(&self, ctx: &AppContext) -> bool {
        self.view.as_ref(ctx).is_being_dragged()
    }
}

pub struct EnvironmentRuntimePlaceholderView {
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    window_id: WindowId,
    pane_id: Option<PaneId>,
}

impl EnvironmentRuntimePlaceholderView {
    pub fn new(window_id: WindowId, ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_ctx| {
            PaneConfiguration::new(crate::t!("environment-runtime-placeholder-title"))
        });

        Self {
            pane_configuration,
            focus_handle: None,
            window_id,
            pane_id: None,
        }
    }

    fn set_pane_id(&mut self, pane_id: PaneId) {
        self.pane_id = Some(pane_id);
    }

    fn lifecycle_state(&self, app: &AppContext) -> Option<EnvironmentLifecycleState> {
        self.pane_id.and_then(|pane_id| {
            WorkspaceRegistry::as_ref(app)
                .get(self.window_id, app)
                .and_then(|workspace| {
                    workspace
                        .as_ref(app)
                        .environment_snapshot_for_pane_id(pane_id, app)
                })
                .map(|environment| environment.lifecycle_state)
        })
    }

    fn placeholder_copy(lifecycle_state: Option<&EnvironmentLifecycleState>) -> (String, String) {
        match lifecycle_state {
            Some(EnvironmentLifecycleState::Connected) => (
                crate::t!("environment-runtime-placeholder-opening"),
                crate::t!("environment-runtime-placeholder-opening-detail"),
            ),
            Some(EnvironmentLifecycleState::Dormant) => (
                crate::t!("environment-runtime-placeholder-dormant"),
                crate::t!("environment-runtime-placeholder-dormant-detail"),
            ),
            Some(EnvironmentLifecycleState::Connecting) => (
                crate::t!("environment-runtime-placeholder-empty"),
                crate::t!("environment-runtime-placeholder-empty-detail"),
            ),
            Some(EnvironmentLifecycleState::Installing) => (
                crate::t!("environment-runtime-placeholder-installing"),
                crate::t!("environment-runtime-placeholder-installing-detail"),
            ),
            Some(EnvironmentLifecycleState::Reconnecting) => (
                crate::t!("environment-runtime-placeholder-reconnecting"),
                crate::t!("environment-runtime-placeholder-reconnecting-detail"),
            ),
            Some(EnvironmentLifecycleState::Error) => (
                crate::t!("environment-runtime-placeholder-error"),
                crate::t!("environment-runtime-placeholder-error-detail"),
            ),
            None => (
                crate::t!("environment-runtime-placeholder-dormant"),
                crate::t!("environment-runtime-placeholder-dormant-detail"),
            ),
        }
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }
}

impl Entity for EnvironmentRuntimePlaceholderView {
    type Event = PaneEvent;
}

impl TypedActionView for EnvironmentRuntimePlaceholderView {
    type Action = ();
}

impl View for EnvironmentRuntimePlaceholderView {
    fn ui_name() -> &'static str {
        "EnvironmentRuntimePlaceholderView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let color = appearance
            .theme()
            .sub_text_color(appearance.theme().background());
        let lifecycle_state = self.lifecycle_state(app);
        let (title, detail) = Self::placeholder_copy(lifecycle_state.as_ref());
        let title_color = if matches!(lifecycle_state, Some(EnvironmentLifecycleState::Error)) {
            appearance.theme().ui_error_color().into()
        } else {
            color.into()
        };
        let font_family = appearance.ui_font_family();

        Align::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Text::new_inline(title, font_family, 13.)
                        .with_color(title_color)
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new_inline(detail, font_family, 12.)
                            .with_color(color.into())
                            .finish(),
                    )
                    .with_margin_top(4.)
                    .finish(),
                )
                .finish(),
        )
        .finish()
    }
}

impl BackingView for EnvironmentRuntimePlaceholderView {
    type PaneHeaderOverflowMenuAction = ();
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        _action: &Self::PaneHeaderOverflowMenuAction,
        _ctx: &mut ViewContext<Self>,
    ) {
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(PaneEvent::Close);
    }

    fn focus_contents(&mut self, _ctx: &mut ViewContext<Self>) {}

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext,
        _app: &AppContext,
    ) -> view::HeaderContent {
        view::HeaderContent::simple(crate::t!("environment-runtime-placeholder-title"))
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

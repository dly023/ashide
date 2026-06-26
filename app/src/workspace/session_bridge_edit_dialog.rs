use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::color::coloru_with_opacity;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::Fill as ThemeFill;
use warpui::{
    elements::{
        Align, Border, ChildAnchor, ChildView, Clipped, ClippedScrollStateHandle,
        ClippedScrollable, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element,
        Empty, Expanded, Flex, Hoverable, MainAxisSize, MouseStateHandle, OffsetPositioning,
        Padding, ParentAnchor, ParentElement, ParentOffsetBounds, Radius, ScrollbarWidth, Stack,
        Text,
    },
    fonts::{Properties, Weight},
    keymap::{FixedBinding, Keystroke},
    platform::Cursor,
    ui_components::components::{UiComponent, UiComponentStyles},
    AppContext, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use crate::{
    ai::agent::conversation::AIConversationId,
    appearance::Appearance,
    editor::{
        EditorOptions, EditorView, EnterAction, EnterSettings, Event as EditorEvent,
        PropagateAndNoOpEscapeKey,
    },
    session_bridge::ir::{SessionIr, SessionMessageIr},
    terminal::CLIAgent,
    ui_components::dialog::{dialog_styles, Dialog},
    ui_components::icons::Icon as UiIcon,
    view_components::action_button::{
        ActionButton, ButtonSize, KeystrokeSource, NakedTheme, PrimaryTheme,
    },
    workspace::SessionBridgeForkTarget,
};

const DIALOG_WIDTH: f32 = 760.;
const MESSAGE_LIST_HEIGHT: f32 = 390.;
const MESSAGE_EDITOR_HEIGHT: f32 = 118.;
const FIELD_PADDING: f32 = 10.;
const TARGET_PILL_HEIGHT: f32 = 30.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionBridgeEditInputError {
    EmptyDraft,
    MissingTarget,
}

impl SessionBridgeEditInputError {
    fn localized_message(self) -> String {
        match self {
            SessionBridgeEditInputError::EmptyDraft => {
                crate::t!("workspace-session-bridge-edit-error-empty-draft")
            }
            SessionBridgeEditInputError::MissingTarget => {
                crate::t!("workspace-session-bridge-edit-error-missing-target")
            }
        }
    }
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([
        FixedBinding::new(
            "escape",
            SessionBridgeEditDialogAction::Cancel,
            id!(SessionBridgeEditDialog::ui_name()),
        ),
        FixedBinding::new(
            "cmdorctrl-enter",
            SessionBridgeEditDialogAction::Confirm,
            id!(SessionBridgeEditDialog::ui_name()),
        ),
    ]);
}

#[derive(Clone)]
pub struct SessionBridgeEditDialogSource {
    pub conversation_id: Option<AIConversationId>,
    pub source_environment_authority_key: Option<String>,
    pub conversation_title: String,
    pub available_fork_targets: Vec<SessionBridgeForkTarget>,
    pub initial_fork_target: SessionBridgeForkTarget,
    pub source_session: SessionIr,
}

#[derive(Debug, Clone)]
pub struct SessionBridgeEditRequest {
    pub conversation_id: Option<AIConversationId>,
    pub source_environment_authority_key: Option<String>,
    pub fork_target: SessionBridgeForkTarget,
    pub source_session: SessionIr,
    pub edited_messages: Vec<SessionMessageIr>,
}

pub enum SessionBridgeEditDialogEvent {
    Confirm { request: SessionBridgeEditRequest },
    Cancel,
}

struct SessionBridgeMessageDraft {
    source_index: usize,
    role: String,
    editor: ViewHandle<EditorView>,
    remove_button: ViewHandle<ActionButton>,
    is_removed: bool,
}

pub struct SessionBridgeEditDialog {
    message_drafts: Vec<SessionBridgeMessageDraft>,
    message_scroll_state: ClippedScrollStateHandle,
    target_mouse_states: Vec<MouseStateHandle>,
    cancel_button: ViewHandle<ActionButton>,
    save_button: ViewHandle<ActionButton>,
    source: Option<SessionBridgeEditDialogSource>,
    selected_fork_target: Option<SessionBridgeForkTarget>,
    validation_error: Option<String>,
}

impl SessionBridgeEditDialog {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new(crate::t!("common-cancel"), NakedTheme)
                .on_click(|ctx| ctx.dispatch_typed_action(SessionBridgeEditDialogAction::Cancel))
        });

        let save_keystroke = Keystroke::parse("cmdorctrl-enter").expect("Valid keystroke");
        let save_button = ctx.add_typed_action_view(|ctx| {
            ActionButton::new(
                crate::t!("workspace-session-bridge-edit-save", target = "Ashide"),
                PrimaryTheme,
            )
            .with_keybinding(KeystrokeSource::Fixed(save_keystroke), ctx)
            .on_click(|ctx| ctx.dispatch_typed_action(SessionBridgeEditDialogAction::Confirm))
        });

        Self {
            message_drafts: Vec::new(),
            message_scroll_state: ClippedScrollStateHandle::default(),
            target_mouse_states: Vec::new(),
            cancel_button,
            save_button,
            source: None,
            selected_fork_target: None,
            validation_error: None,
        }
    }

    pub fn set_source(
        &mut self,
        source: SessionBridgeEditDialogSource,
        ctx: &mut ViewContext<Self>,
    ) {
        let selected_fork_target = if source
            .available_fork_targets
            .contains(&source.initial_fork_target)
        {
            source.initial_fork_target
        } else {
            source
                .available_fork_targets
                .first()
                .copied()
                .unwrap_or(SessionBridgeForkTarget::Ashide)
        };

        self.message_drafts.clear();
        for (index, message) in source.source_session.messages.iter().enumerate() {
            let editor = Self::message_editor(ctx);
            ctx.subscribe_to_view(&editor, |me, _, event, ctx| {
                me.handle_editor_event(event, ctx);
            });
            editor.update(ctx, |editor, ctx| {
                editor.system_reset_buffer_text(&message.text, ctx)
            });
            let remove_button = ctx.add_typed_action_view(move |_| {
                ActionButton::new(
                    crate::t!("workspace-session-bridge-edit-remove-message"),
                    NakedTheme,
                )
                .with_size(ButtonSize::XSmall)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(SessionBridgeEditDialogAction::ToggleMessageRemoved(
                        index,
                    ))
                })
            });
            self.message_drafts.push(SessionBridgeMessageDraft {
                source_index: index,
                role: message.role.clone(),
                editor,
                remove_button,
                is_removed: false,
            });
        }

        self.target_mouse_states = source
            .available_fork_targets
            .iter()
            .map(|_| MouseStateHandle::default())
            .collect();
        self.selected_fork_target = Some(selected_fork_target);
        self.update_save_button_label(selected_fork_target, ctx);
        self.source = Some(source);
        self.validation_error = None;
        self.focus_first_message_editor(ctx);
        ctx.notify();
    }

    fn message_editor(ctx: &mut ViewContext<Self>) -> ViewHandle<EditorView> {
        let options = EditorOptions {
            enter_settings: EnterSettings {
                enter: EnterAction::InsertNewLineIfMultiLine,
                ..Default::default()
            },
            propagate_and_no_op_escape_key: PropagateAndNoOpEscapeKey::HandleFirst,
            soft_wrap: true,
            ..Default::default()
        };
        ctx.add_typed_action_view(|ctx| EditorView::new(options, ctx))
    }

    fn focus_first_message_editor(&self, ctx: &mut ViewContext<Self>) {
        if let Some(draft) = self
            .message_drafts
            .iter()
            .find(|draft| !draft.is_removed)
            .or_else(|| self.message_drafts.first())
        {
            ctx.focus(&draft.editor);
        }
    }

    fn update_save_button_label(
        &self,
        fork_target: SessionBridgeForkTarget,
        ctx: &mut ViewContext<Self>,
    ) {
        self.save_button.update(ctx, |button, ctx| {
            button.set_label(
                crate::t!(
                    "workspace-session-bridge-edit-save",
                    target = fork_target.display_label()
                ),
                ctx,
            );
        });
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::CmdEnter => self.submit(ctx),
            EditorEvent::Escape => ctx.emit(SessionBridgeEditDialogEvent::Cancel),
            EditorEvent::Edited(_)
            | EditorEvent::BufferReplaced
            | EditorEvent::BufferReinitialized => {
                if self.validation_error.take().is_some() {
                    ctx.notify();
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self, ctx: &mut ViewContext<Self>) {
        match self.request(ctx) {
            Ok(request) => ctx.emit(SessionBridgeEditDialogEvent::Confirm { request }),
            Err(error) => {
                self.validation_error = Some(error);
                ctx.notify();
            }
        }
    }

    fn request(&self, ctx: &AppContext) -> Result<SessionBridgeEditRequest, String> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| crate::t!("workspace-session-bridge-edit-error-missing-conversation"))?;
        let fork_target = self
            .selected_fork_target
            .ok_or(SessionBridgeEditInputError::MissingTarget)
            .map_err(SessionBridgeEditInputError::localized_message)?;

        let mut edited_messages = Vec::new();
        for draft in &self.message_drafts {
            if draft.is_removed {
                continue;
            }
            let Some(source_message) = source.source_session.messages.get(draft.source_index)
            else {
                continue;
            };
            let mut message = source_message.clone();
            message.role = draft.role.clone();
            message.text = draft.editor.as_ref(ctx).buffer_text(ctx);
            edited_messages.push(message);
        }

        if edited_messages.is_empty() {
            return Err(SessionBridgeEditInputError::EmptyDraft.localized_message());
        }

        Ok(SessionBridgeEditRequest {
            conversation_id: source.conversation_id,
            source_environment_authority_key: source.source_environment_authority_key.clone(),
            fork_target,
            source_session: source.source_session.clone(),
            edited_messages,
        })
    }

    fn role_label(role: &str) -> String {
        match role {
            "user" => crate::t!("workspace-session-bridge-edit-role-user"),
            "assistant" => crate::t!("workspace-session-bridge-edit-role-assistant"),
            "system" => crate::t!("workspace-session-bridge-edit-role-system"),
            other => other.to_owned(),
        }
    }

    fn source_default_fork_target(source_session: &SessionIr) -> SessionBridgeForkTarget {
        match source_session.source.as_str() {
            "codex" => SessionBridgeForkTarget::Agent(CLIAgent::Codex),
            "claude" => SessionBridgeForkTarget::Agent(CLIAgent::Claude),
            "ashide" => SessionBridgeForkTarget::Ashide,
            _ => SessionBridgeForkTarget::Ashide,
        }
    }

    pub fn default_fork_target_for_source(source_session: &SessionIr) -> SessionBridgeForkTarget {
        Self::source_default_fork_target(source_session)
    }

    fn render_target_selector(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let Some(source) = self.source.as_ref() else {
            return Empty::new().finish();
        };
        let selected_fork_target = self
            .selected_fork_target
            .unwrap_or(source.initial_fork_target);

        let label = Text::new_inline(
            crate::t!("workspace-session-bridge-edit-target-label"),
            appearance.ui_font_family(),
            12.,
        )
        .with_color(theme.sub_text_color(theme.surface_1()).into())
        .finish();

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_spacing(8.)
            .with_child(label);

        for (index, fork_target) in source.available_fork_targets.iter().copied().enumerate() {
            let is_selected = fork_target == selected_fork_target;
            let mouse_state = self
                .target_mouse_states
                .get(index)
                .cloned()
                .unwrap_or_default();
            row.add_child(self.render_target_pill(fork_target, is_selected, mouse_state, app));
        }

        Container::new(row.finish())
            .with_padding_bottom(12.)
            .finish()
    }

    fn render_target_pill(
        &self,
        fork_target: SessionBridgeForkTarget,
        is_selected: bool,
        mouse_state: MouseStateHandle,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let main_text: ColorU = theme.main_text_color(theme.surface_1()).into();
        let accent: ColorU = theme.accent().into();
        let label = fork_target.display_label().to_owned();
        let content = move |hovered: bool| {
            let text_color = if is_selected { accent } else { main_text };
            let icon = if is_selected {
                ConstrainedBox::new(
                    UiIcon::Check
                        .to_warpui_icon(ThemeFill::Solid(text_color))
                        .finish(),
                )
                .with_width(12.)
                .with_height(12.)
                .finish()
            } else {
                Empty::new().finish()
            };
            let row = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_spacing(if is_selected { 5. } else { 0. })
                .with_child(icon)
                .with_child(
                    Text::new_inline(label.clone(), appearance.ui_font_family(), 12.)
                        .with_color(ThemeFill::Solid(text_color).into())
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .finish(),
                )
                .finish();
            let border_color = if is_selected {
                accent
            } else {
                internal_colors::neutral_4(theme)
            };
            let background = if is_selected {
                ThemeFill::Solid(coloru_with_opacity(accent, 10))
            } else if hovered {
                internal_colors::fg_overlay_2(theme)
            } else {
                internal_colors::fg_overlay_1(theme)
            };
            Container::new(row)
                .with_padding(Padding::uniform(0.).with_left(10.).with_right(10.))
                .with_background(background)
                .with_border(Border::all(1.).with_border_color(border_color))
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(999.)))
                .finish()
        };

        ConstrainedBox::new(
            Hoverable::new(mouse_state, move |state| content(state.is_hovered()))
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(SessionBridgeEditDialogAction::SelectForkTarget(
                        fork_target,
                    ));
                })
                .finish(),
        )
        .with_height(TARGET_PILL_HEIGHT)
        .finish()
    }

    fn render_message_drafts(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let label = Text::new_inline(
            crate::t!("workspace-session-bridge-edit-message-draft"),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(theme.sub_text_color(theme.surface_1()).into())
        .finish();

        let mut cards = Flex::column().with_spacing(8.);
        for draft in &self.message_drafts {
            cards.add_child(self.render_message_card(draft, app));
        }

        let list = ClippedScrollable::vertical(
            self.message_scroll_state.clone(),
            cards.finish(),
            ScrollbarWidth::Auto,
            theme.disabled_text_color(theme.surface_1()).into(),
            theme.main_text_color(theme.surface_1()).into(),
            theme.surface_1().into(),
        )
        .with_overlayed_scrollbar()
        .finish();

        let list = ConstrainedBox::new(list)
            .with_height(MESSAGE_LIST_HEIGHT)
            .finish();

        Container::new(
            Flex::column()
                .with_spacing(8.)
                .with_child(label)
                .with_child(list)
                .finish(),
        )
        .with_padding_bottom(14.)
        .finish()
    }

    fn render_message_card(
        &self,
        draft: &SessionBridgeMessageDraft,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let sub_text: ColorU = theme.sub_text_color(theme.surface_1()).into();
        let main_text: ColorU = theme.main_text_color(theme.surface_1()).into();
        let role_label = Self::role_label(&draft.role);
        let header_title = Text::new_inline(
            format!("#{} · {role_label}", draft.source_index + 1),
            appearance.ui_font_family(),
            12.,
        )
        .with_color(ThemeFill::Solid(coloru_with_opacity(main_text, 88)).into())
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish();
        let status_text = if draft.is_removed {
            crate::t!("workspace-session-bridge-edit-message-removed")
        } else {
            crate::t!("workspace-session-bridge-edit-message-kept")
        };
        let status = Text::new_inline(status_text, appearance.ui_font_family(), 11.)
            .with_color(ThemeFill::Solid(coloru_with_opacity(sub_text, 78)).into())
            .finish();
        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., header_title).finish())
            .with_child(Container::new(status).with_margin_right(8.).finish())
            .with_child(ChildView::new(&draft.remove_button).finish())
            .finish();

        let body = if draft.is_removed {
            Container::new(
                Text::new_inline(
                    crate::t!("workspace-session-bridge-edit-message-removed-help"),
                    appearance.ui_font_family(),
                    12.,
                )
                .with_color(ThemeFill::Solid(coloru_with_opacity(sub_text, 76)).into())
                .finish(),
            )
            .with_padding_top(8.)
            .finish()
        } else {
            Container::new(
                ConstrainedBox::new(Clipped::new(ChildView::new(&draft.editor).finish()).finish())
                    .with_height(MESSAGE_EDITOR_HEIGHT)
                    .finish(),
            )
            .with_uniform_padding(FIELD_PADDING)
            .with_margin_top(8.)
            .with_background(theme.background())
            .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .finish()
        };

        let background = if draft.is_removed {
            internal_colors::fg_overlay_1(theme)
        } else {
            theme.surface_2()
        };
        Container::new(
            Flex::column()
                .with_main_axis_size(MainAxisSize::Min)
                .with_child(header)
                .with_child(body)
                .finish(),
        )
        .with_uniform_padding(10.)
        .with_background(background)
        .with_border(Border::all(1.).with_border_fill(theme.surface_3()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
        .finish()
    }

    fn render_validation_error(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let error = self.validation_error.as_ref()?;
        let appearance = Appearance::as_ref(app);
        Some(
            Container::new(
                Text::new_inline(error.clone(), appearance.ui_font_family(), 13.)
                    .with_color(appearance.theme().ansi_fg_red().into())
                    .finish(),
            )
            .with_padding_bottom(12.)
            .finish(),
        )
    }
}

impl Entity for SessionBridgeEditDialog {
    type Event = SessionBridgeEditDialogEvent;
}

impl View for SessionBridgeEditDialog {
    fn ui_name() -> &'static str {
        "SessionBridgeEditDialog"
    }

    fn on_focus(&mut self, _focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        self.focus_first_message_editor(ctx);
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let title = self
            .source
            .as_ref()
            .map(|source| {
                crate::t!(
                    "workspace-session-bridge-edit-title-with-name",
                    name = source.conversation_title.as_str()
                )
            })
            .unwrap_or_else(|| crate::t!("workspace-session-bridge-edit-title"));

        let mut form = Flex::column();
        form.add_child(self.render_target_selector(app));
        form.add_child(self.render_message_drafts(app));
        if let Some(error) = self.render_validation_error(app) {
            form.add_child(error);
        }

        let cancel_button = Container::new(ChildView::new(&self.cancel_button).finish())
            .with_margin_right(12.)
            .finish();
        let dialog = Dialog::new(
            title,
            None,
            UiComponentStyles {
                width: Some(DIALOG_WIDTH),
                ..dialog_styles(appearance)
            },
        )
        .with_child(form.finish())
        .with_bottom_row_child(cancel_button)
        .with_bottom_row_child(ChildView::new(&self.save_button).finish())
        .build()
        .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            dialog,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );

        Container::new(Align::new(stack.finish()).finish())
            .with_background_color(ThemeFill::blur().into())
            .with_corner_radius(app.windows().window_corner_radius())
            .finish()
    }
}

#[derive(Debug)]
pub enum SessionBridgeEditDialogAction {
    Confirm,
    Cancel,
    SelectForkTarget(SessionBridgeForkTarget),
    ToggleMessageRemoved(usize),
}

impl TypedActionView for SessionBridgeEditDialog {
    type Action = SessionBridgeEditDialogAction;

    fn handle_action(
        &mut self,
        action: &SessionBridgeEditDialogAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            SessionBridgeEditDialogAction::Confirm => self.submit(ctx),
            SessionBridgeEditDialogAction::Cancel => {
                ctx.emit(SessionBridgeEditDialogEvent::Cancel);
            }
            SessionBridgeEditDialogAction::SelectForkTarget(fork_target) => {
                self.selected_fork_target = Some(*fork_target);
                self.update_save_button_label(*fork_target, ctx);
                if self.validation_error.take().is_some() {
                    ctx.notify();
                }
                ctx.notify();
            }
            SessionBridgeEditDialogAction::ToggleMessageRemoved(index) => {
                if let Some(draft) = self.message_drafts.get_mut(*index) {
                    draft.is_removed = !draft.is_removed;
                    let label = if draft.is_removed {
                        crate::t!("workspace-session-bridge-edit-restore-message")
                    } else {
                        crate::t!("workspace-session-bridge-edit-remove-message")
                    };
                    draft.remove_button.update(ctx, |button, ctx| {
                        button.set_label(label, ctx);
                    });
                    if self.validation_error.take().is_some() {
                        ctx.notify();
                    }
                    ctx.notify();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SessionBridgeEditDialog, SessionBridgeEditDialogAction, SessionBridgeEditDialogSource,
        SessionBridgeEditInputError,
    };
    use crate::session_bridge::ir::{SessionIr, SessionMessageIr, SessionTimestamp};
    use crate::terminal::CLIAgent;
    use crate::{
        notebooks::editor::keys::NotebookKeybindings,
        object_store::model::persistence::ObjectStoreModel,
        settings_view::keybindings::KeybindingChangedNotifier,
        test_util::settings::initialize_settings_for_tests,
        vim_registers::VimRegisters,
        workspace::{sync_inputs::SyncedInputState, ActiveSession, SessionBridgeForkTarget},
        workspaces::user_workspaces::UserWorkspaces,
        AuthStateProvider,
    };
    use warp_core::ui::appearance::Appearance;
    use warpui::{platform::WindowStyle, App, TypedActionView, ViewHandle};

    fn source_session() -> SessionIr {
        let mut source_session = SessionIr::new_ashide("source-session");
        source_session.title = "Editable source".to_owned();
        source_session.messages = vec![
            SessionMessageIr {
                role: "user".to_owned(),
                text: "hello".to_owned(),
                timestamp: Some(SessionTimestamp::Integer(1)),
            },
            SessionMessageIr {
                role: "assistant".to_owned(),
                text: "world".to_owned(),
                timestamp: Some(SessionTimestamp::Integer(2)),
            },
        ];
        source_session
    }

    fn initialize_dialog(app: &mut App) -> ViewHandle<SessionBridgeEditDialog> {
        initialize_settings_for_tests(app);
        app.add_singleton_model(|_| Appearance::mock());
        app.add_singleton_model(|_| SyncedInputState::mock());
        app.add_singleton_model(|_| VimRegisters::new());
        app.add_singleton_model(|_| KeybindingChangedNotifier::mock());
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(ObjectStoreModel::mock);
        app.add_singleton_model(|_| ActiveSession::default());
        app.add_singleton_model(NotebookKeybindings::new);
        app.add_singleton_model(|ctx| UserWorkspaces::mock(vec![], ctx));

        let (_, dialog) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            SessionBridgeEditDialog::new(ctx)
        });
        dialog
    }

    #[test]
    fn edit_request_uses_selected_target_and_message_cards() {
        App::test((), |mut app| async move {
            let dialog = initialize_dialog(&mut app);
            dialog.update(&mut app, |dialog, ctx| {
                dialog.set_source(
                    SessionBridgeEditDialogSource {
                        conversation_id: None,
                        source_environment_authority_key: Some("ssh:dnyx216".to_owned()),
                        conversation_title: "Editable source".to_owned(),
                        available_fork_targets: vec![
                            SessionBridgeForkTarget::Ashide,
                            SessionBridgeForkTarget::Agent(CLIAgent::Codex),
                            SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                        ],
                        initial_fork_target: SessionBridgeForkTarget::Ashide,
                        source_session: source_session(),
                    },
                    ctx,
                );
                dialog.handle_action(
                    &SessionBridgeEditDialogAction::SelectForkTarget(
                        SessionBridgeForkTarget::Agent(CLIAgent::Claude),
                    ),
                    ctx,
                );
                dialog.message_drafts[0].editor.update(ctx, |editor, ctx| {
                    editor.system_reset_buffer_text("edited hello", ctx)
                });
                dialog.handle_action(&SessionBridgeEditDialogAction::ToggleMessageRemoved(1), ctx);

                let request = dialog.request(ctx).unwrap();

                assert_eq!(
                    request.fork_target,
                    SessionBridgeForkTarget::Agent(CLIAgent::Claude)
                );
                assert_eq!(request.edited_messages.len(), 1);
                assert_eq!(request.edited_messages[0].role, "user");
                assert_eq!(request.edited_messages[0].text, "edited hello");
                assert_eq!(
                    request.edited_messages[0].timestamp,
                    Some(SessionTimestamp::Integer(1))
                );
                assert_eq!(
                    request.source_environment_authority_key.as_deref(),
                    Some("ssh:dnyx216")
                );
            });
        });
    }

    #[test]
    fn edit_request_rejects_removing_every_message() {
        App::test((), |mut app| async move {
            let dialog = initialize_dialog(&mut app);
            dialog.update(&mut app, |dialog, ctx| {
                dialog.set_source(
                    SessionBridgeEditDialogSource {
                        conversation_id: None,
                        source_environment_authority_key: None,
                        conversation_title: "Editable source".to_owned(),
                        available_fork_targets: vec![SessionBridgeForkTarget::Ashide],
                        initial_fork_target: SessionBridgeForkTarget::Ashide,
                        source_session: source_session(),
                    },
                    ctx,
                );
                dialog.handle_action(&SessionBridgeEditDialogAction::ToggleMessageRemoved(0), ctx);
                dialog.handle_action(&SessionBridgeEditDialogAction::ToggleMessageRemoved(1), ctx);

                assert_eq!(
                    dialog.request(ctx).unwrap_err(),
                    SessionBridgeEditInputError::EmptyDraft.localized_message()
                );
            });
        });
    }

    #[test]
    fn default_target_tracks_source_agent() {
        let mut codex_session = source_session();
        codex_session.source = "codex".to_owned();
        assert_eq!(
            SessionBridgeEditDialog::default_fork_target_for_source(&codex_session),
            SessionBridgeForkTarget::Agent(CLIAgent::Codex)
        );

        let mut claude_session = source_session();
        claude_session.source = "claude".to_owned();
        assert_eq!(
            SessionBridgeEditDialog::default_fork_target_for_source(&claude_session),
            SessionBridgeForkTarget::Agent(CLIAgent::Claude)
        );

        let ashide_session = source_session();
        assert_eq!(
            SessionBridgeEditDialog::default_fork_target_for_source(&ashide_session),
            SessionBridgeForkTarget::Ashide
        );
    }
}

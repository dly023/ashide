use anyhow::Context;
use async_channel::Sender;
use lazy_static::lazy_static;
use regex::Regex;
use settings::Setting as _;
use std::{sync::Arc, time::Duration};

use warp_editor::{
    editor::NavigationKey,
    model::{CoreEditorModel, RichTextEditorModel},
};
use warpui::{
    accessibility::{AccessibilityContent, WarpA11yRole},
    elements::{
        Align, Clipped, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Empty,
        EventHandler, Flex, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement,
        SavePosition, Shrinkable, Stack,
    },
    keymap::{EditableBinding, FixedBinding},
    presenter::ChildView,
    r#async::SpawnedFutureHandle,
    ui_components::{
        button::ButtonVariant,
        components::{UiComponent, UiComponentStyles},
    },
    AppContext, BlurContext, Element, Entity, FocusContext, ModelAsRef, ModelHandle,
    SingletonEntity, TypedActionView, View, ViewContext, ViewHandle, WindowId,
};

use crate::{
    ai::{
        blocklist::secret_redaction::find_secrets_in_text,
        document::ai_document_model::AIDocumentId,
    },
    appearance::Appearance,
    cmd_or_ctrl_shift,
    drive::{
        export::ExportManager, items::LocalDriveItemId, LocalDriveObjectSettings, ObjectTypeAndId,
    },
    editor::{
        EditOrigin, EditorView, Event as EditorEvent, InteractionState,
        PropagateAndNoOpNavigationKeys, SingleLineEditorOptions, TextColors, TextOptions,
    },
    menu::{MenuItem, MenuItemFields},
    notebooks::{
        editor::{model::NotebooksEditorModel, rich_text_styles},
        NotebookObject,
    },
    object_store::ids::ObjectStoreId,
    object_store::{
        grab_edit_access_modal::{GrabEditAccessModal, GrabEditAccessModalEvent},
        model::{
            persistence::{ObjectStoreEvent, ObjectStoreModel, UpdateSource},
            view::{Editor, EditorState},
        },
        update_manager::UpdateManager,
        ObjectType, Owner, StoredObject, StoredObjectEventEntrypoint,
    },
    pane_group::{
        focus_state::{PaneFocusHandle, PaneGroupFocusEvent},
        pane::view,
        BackingView, PaneConfiguration, PaneEvent,
    },
    report_if_error, safe_info,
    settings::{
        decrease_notebook_font_size, increase_notebook_font_size, FontSettings,
        FontSettingsChangedEvent, NotebookFontSize,
    },
    terminal::safe_mode_settings::get_secret_obfuscation_mode,
    throttle::throttle,
    ui_components::icons,
    util::bindings::{self, CustomAction},
    view_components::{DismissibleToast, ToastType},
    workflows::{WorkflowSource, WorkflowType},
    workspace::ToastStack,
    workspaces::user_workspaces::UserWorkspaces,
};

use self::details_bar::DetailsBar;

use super::{
    active_notebook_data::{
        ActiveNotebook, ActiveNotebookData, ActiveNotebookDataEvent, Mode, SavingStatus,
        TrashStatus,
    },
    context_menu::{
        show_rich_editor_context_menu, show_text_editor_context_menu, ContextMenuAction,
        ContextMenuState,
    },
    editor::{
        view::{EditorViewEvent, RichTextEditorConfig, RichTextEditorView},
        NotebookWorkflow,
    },
    link::{NotebookLinks, SessionSource},
    styles, NotebookId, NotebookLocation, NotebookObjectModel,
};

mod details_bar;

#[cfg(test)]
#[path = "notebook_tests.rs"]
mod tests;

const EDIT_BUTTON_MARGIN: f32 = 6.;
const HEADER_MARGIN: f32 = 15.;
const BANNER_VERTICAL_MARGIN: f32 = 10.;

/// The frequency at which we check for modifications and persist the notebook to
/// the local object store.
const SAVE_PERIOD: Duration = Duration::from_secs(2);

lazy_static! {
    // This is used to replace any backslash followed by a punctuation character with just the punctuation character.
    static ref ESCAPE_PUNCTUATION_REGEX: Regex =
        Regex::new(r"\\([[:punct:]])").expect("Escape punctuation regex should be valid");
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_editable_bindings([
        EditableBinding::new(
            "notebookview:increase_font_size",
            crate::t!("keybinding-desc-notebook-increase-font-size"),
            NotebookAction::IncreaseFontSize,
        )
        .with_context_predicate(id!("NotebookView") & id!("NotMatchNotebookToMonospaceSize"))
        .with_group(bindings::BindingGroup::Settings.as_str())
        .with_key_binding("cmdorctrl-="),
        EditableBinding::new(
            "notebookview:decrease_font_size",
            crate::t!("keybinding-desc-notebook-decrease-font-size"),
            NotebookAction::DecreaseFontSize,
        )
        .with_context_predicate(id!("NotebookView") & id!("NotMatchNotebookToMonospaceSize"))
        .with_group(bindings::BindingGroup::Settings.as_str())
        .with_key_binding("cmdorctrl--"),
        EditableBinding::new(
            "notebookview:reset_font_size",
            crate::t!("keybinding-desc-notebook-reset-font-size"),
            NotebookAction::ResetFontSize,
        )
        .with_context_predicate(id!("NotebookView") & id!("NotMatchNotebookToMonospaceSize"))
        .with_group(bindings::BindingGroup::Settings.as_str())
        .with_custom_action(CustomAction::ResetFontSize),
        EditableBinding::new(
            "notebookview:focus_terminal_input",
            crate::t!("keybinding-desc-notebook-focus-terminal-input"),
            NotebookAction::FocusTerminalInput,
        )
        .with_context_predicate(id!("NotebookView"))
        .with_key_binding(cmd_or_ctrl_shift("l")),
    ]);
    app.register_fixed_bindings([
        FixedBinding::new(
            "alt-enter",
            NotebookAction::ToggleMode,
            id!("NotebookView") & id!("NotebookIsEditable"),
        ),
        FixedBinding::custom(
            CustomAction::IncreaseFontSize,
            NotebookAction::IncreaseFontSize,
            crate::t!("keybinding-desc-notebook-fb-increase-font-size"),
            id!("NotebookView") & id!("NotMatchNotebookToMonospaceSize"),
        )
        .with_group(bindings::BindingGroup::Settings.as_str()),
        FixedBinding::custom(
            CustomAction::DecreaseFontSize,
            NotebookAction::DecreaseFontSize,
            crate::t!("keybinding-desc-notebook-fb-decrease-font-size"),
            id!("NotebookView") & id!("NotMatchNotebookToMonospaceSize"),
        )
        .with_group(bindings::BindingGroup::Settings.as_str()),
    ]);
}

struct NotebookUpdateRequestDebounceArg {}

#[derive(Default)]
struct ButtonMouseStates {
    conflict_resolution_refresh_button: MouseStateHandle,
    conflict_resolution_copy_all_button: MouseStateHandle,
    restore_from_trash_button: MouseStateHandle,
}

#[derive(Clone, Copy)]
enum NotebookLocalSaveNotice {
    InConflict,
    FeatureNotAvailable,
}

/// A view for viewing, executing, and editing an Ashide notebook backed by the
/// local object store.
pub struct NotebookView {
    /// This is a stateful component that shows information about the notebook like its location
    /// breadcrumbs and the current editor. It's shown immediately above the title editor.
    details_bar: DetailsBar,
    title: ViewHandle<EditorView>,
    input: ViewHandle<RichTextEditorView>,
    grab_edit_access_modal: ViewHandle<GrabEditAccessModal>,
    focused: bool,
    last_focused_component: FocusedComponent,
    active_notebook_data: ModelHandle<ActiveNotebookData>,
    button_mouse_states: ButtonMouseStates,
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    links: ModelHandle<NotebookLinks>,
    context_menu: ContextMenuState<Self>,

    /// Whether or not there are un-saved content edits.
    content_is_dirty: bool,
    /// Whether or not there are un-saved title edits.
    title_is_dirty: bool,
    /// Sender for requesting throttled saves.
    save_tx: Sender<NotebookUpdateRequestDebounceArg>,

    /// Save position for the bounds of this view.
    view_position_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotebookEvent {
    RunWorkflow {
        workflow: Arc<WorkflowType>,
        source: WorkflowSource,
    },
    EditWorkflow(ObjectStoreId),
    ViewInLocalDrive(LocalDriveItemId),
    Pane(PaneEvent),
    AttachPlanAsContext(AIDocumentId),
}

impl From<PaneEvent> for NotebookEvent {
    fn from(event: PaneEvent) -> Self {
        NotebookEvent::Pane(event)
    }
}

#[derive(Debug, Clone)]
pub enum NotebookAction {
    Focus,
    ToggleMode,
    Close,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ConflictResolutionBannerRefreshClicked,
    FocusTerminalInput,
    ViewInLocalDrive(LocalDriveItemId),
    ContextMenu(ContextMenuAction), // right click context menu
    Duplicate,
    Trash,
    Untrash,
    CopyToClipboard,
    Export,
    AttachPlanAsContext(AIDocumentId),
}

impl From<ContextMenuAction> for NotebookAction {
    fn from(action: ContextMenuAction) -> Self {
        NotebookAction::ContextMenu(action)
    }
}

/// A focusable component of the notebook view. This is used to restore focus to the right
/// component when re-focusing the notebook view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedComponent {
    /// The title editor.
    Title,
    /// The body/input editor.
    Input,
}

impl NotebookView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.handle_appearance_change(ctx)
        });

        ctx.subscribe_to_model(&FontSettings::handle(ctx), |me, _, event, ctx| {
            if matches!(
                event,
                FontSettingsChangedEvent::NotebookFontSize { .. }
                    | FontSettingsChangedEvent::MatchNotebookToMonospaceFontSize { .. }
            ) {
                me.handle_appearance_change(ctx)
            }
        });

        let active_notebook_data = ctx.add_model(ActiveNotebookData::new);
        ctx.subscribe_to_model(&active_notebook_data, Self::handle_active_notebook_event);
        ctx.observe(&active_notebook_data, Self::handle_active_notebook_change);

        let window_id = ctx.window_id();
        let links = ctx.add_model(|ctx| NotebookLinks::new(SessionSource::Active(window_id), ctx));

        let title = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let font_settings = FontSettings::as_ref(ctx);

            let options = SingleLineEditorOptions {
                text: TextOptions {
                    font_family_override: Some(appearance.ui_font_family()),
                    font_size_override: Some(styles::title_font_size(font_settings)),
                    font_properties_override: Some(styles::TITLE_FONT_PROPERTIES),
                    text_colors_override: Some(title_text_colors(appearance)),
                },
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text(Self::untitled_title(), ctx);
            editor
        });
        ctx.subscribe_to_view(&title, |notebook, _, event, ctx| {
            notebook.handle_title_editor_event(event, ctx);
        });

        let view_position_id = format!("notebook_view_{}", ctx.view_id());
        let input = ctx.add_typed_action_view(|ctx| {
            let editor_model = ctx.add_model(|ctx| {
                let styles = rich_text_styles(Appearance::as_ref(ctx), FontSettings::as_ref(ctx));
                NotebooksEditorModel::new(styles, window_id, ctx)
            });
            let editor = RichTextEditorView::new(
                view_position_id.clone(),
                editor_model,
                links.clone(),
                RichTextEditorConfig {
                    max_width: Some(styles::notebook_editor_max_width()),
                    ..Default::default()
                },
                ctx,
            );
            ctx.focus_self();
            editor
        });
        ctx.subscribe_to_view(&input, |notebook, _, event, ctx| {
            notebook.handle_input_editor_event(event, ctx);
        });

        let grab_edit_access_modal = ctx.add_typed_action_view(|_| GrabEditAccessModal::new());
        ctx.subscribe_to_view(&grab_edit_access_modal, |notebook, _, event, ctx| {
            notebook.handle_grab_edit_access_modal_event(event, ctx);
        });

        let user_workspaces = UserWorkspaces::handle(ctx);
        ctx.observe(&user_workspaces, Self::on_user_workspaces_update);

        let object_store_model = ObjectStoreModel::handle(ctx);
        ctx.subscribe_to_model(&object_store_model, |notebook, _handle, event, ctx| {
            notebook.handle_object_store_event(event, ctx);
        });

        let (save_tx, save_rx) = async_channel::unbounded();
        ctx.spawn_stream_local(throttle(SAVE_PERIOD, save_rx), Self::handle_save, |_, _| {});

        let title_str = Self::title_from_editor(&title, ctx);
        let pane_configuration = ctx.add_model(|_ctx| PaneConfiguration::new(title_str));

        let context_menu = ContextMenuState::new(ctx);

        Self {
            details_bar: DetailsBar::new(),
            title,
            input,
            grab_edit_access_modal,
            focused: false,
            last_focused_component: FocusedComponent::Input,
            active_notebook_data,
            links,
            context_menu,
            button_mouse_states: Default::default(),
            pane_configuration,
            focus_handle: None,
            content_is_dirty: false,
            title_is_dirty: false,
            save_tx,
            view_position_id,
        }
    }

    /// Restore focus to the notebook view, by focusing its editor.
    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        // Emit accessibility content for the notebook, rather than the generic text input.
        if let Some(a11y_content) = self.accessibility_contents(ctx) {
            ctx.emit_a11y_content(a11y_content);
        }
        match self.last_focused_component {
            FocusedComponent::Title => self.focus_title(ctx),
            FocusedComponent::Input => self.focus_input(ctx),
        }
    }

    /// Focus the title view.
    fn focus_title(&mut self, ctx: &mut ViewContext<Self>) {
        log::trace!("Focusing notebook title editor");
        self.last_focused_component = FocusedComponent::Title;
        ctx.focus(&self.title);
        ctx.emit(NotebookEvent::Pane(PaneEvent::FocusSelf));
    }

    /// Focus the input editor.
    fn focus_input(&mut self, ctx: &mut ViewContext<Self>) {
        log::trace!("Focusing notebook body editor");
        self.last_focused_component = FocusedComponent::Input;
        ctx.focus(&self.input);
        ctx.emit(NotebookEvent::Pane(PaneEvent::FocusSelf));
    }

    /// Set the interaction states of the title and body editors.
    fn set_editor_interaction_state(
        &self,
        interaction_state: InteractionState,
        ctx: &mut ViewContext<Self>,
    ) {
        self.input.update(ctx, |input, ctx| {
            input.cursor_start(ctx);
            input.set_interaction_state(interaction_state, ctx);
        });
        self.title.update(ctx, |title, ctx| {
            title.set_interaction_state(interaction_state, ctx);
        });
    }

    fn title_from_editor(title_editor: &ViewHandle<EditorView>, app: &AppContext) -> String {
        let mut title = title_editor.as_ref(app).buffer_text(app);
        if title.is_empty() {
            title.push_str(&Self::untitled_title());
        }
        title
    }

    fn untitled_title() -> String {
        let title = crate::t!("common-untitled");
        if title == "common-untitled" {
            "Untitled".to_string()
        } else {
            title
        }
    }

    /// The notebook title. This is pulled from the title editor, and may be more
    /// recent than what's been persisted to the local object store.
    fn title(&self, app: &AppContext) -> String {
        Self::title_from_editor(&self.title, app)
    }

    fn handle_focus_state_event(
        &mut self,
        event: &PaneGroupFocusEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        // For events that change the pane size, rebuild the editor layout to adjust soft-wrapping.
        if matches!(
            event,
            PaneGroupFocusEvent::FocusedPaneMaximizedChanged
                | PaneGroupFocusEvent::InSplitPaneChanged
        ) {
            self.input.update(ctx, |input, ctx| {
                input
                    .model()
                    .update(ctx, |model, ctx| model.rebuild_layout(ctx))
            });
        }
    }

    pub fn pane_configuration(&self) -> &ModelHandle<PaneConfiguration> {
        &self.pane_configuration
    }

    /// Model for resolving and opening links relative to this notebook.
    pub fn links(&self) -> ModelHandle<NotebookLinks> {
        self.links.clone()
    }

    pub fn selected_text(&self, ctx: &AppContext) -> Option<String> {
        let selected_text = self
            .input
            .as_ref(ctx)
            .model()
            .as_ref(ctx)
            .selected_text(ctx);
        if selected_text.is_empty() {
            return None;
        }
        Some(selected_text)
    }

    #[cfg(test)]
    pub fn context_menu(&mut self) -> &mut ContextMenuState<Self> {
        &mut self.context_menu
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn input_editor(&self) -> ViewHandle<RichTextEditorView> {
        self.input.clone()
    }

    #[cfg(test)]
    pub fn title_editor(&self) -> ViewHandle<EditorView> {
        self.title.clone()
    }

    fn on_user_workspaces_update(
        &mut self,
        _user_workspaces: ModelHandle<UserWorkspaces>,
        ctx: &mut ViewContext<Self>,
    ) {
        // TODO Update the notebook after receiving the event from UserWorkspaces model"
        // Update the notebook view if it's a team notebook (assuming there are non-team
        // notebooks?) and there's been changes to it
        self.pane_configuration.update(ctx, |pane_config, ctx| {
            pane_config.refresh_pane_header_overflow_menu_items(ctx)
        });
        ctx.notify();
    }

    /// Handle an event from this notebook's [`ActiveNotebookData`] model.
    fn handle_active_notebook_event(
        &mut self,
        _handle: ModelHandle<ActiveNotebookData>,
        event: &ActiveNotebookDataEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            ActiveNotebookDataEvent::EditorChangedExternally => {
                log::info!("Edit mode stolen");
                self.switch_to_view(ctx);
            }
            ActiveNotebookDataEvent::SwitchedToEditMode => {
                log::info!("Edit mode confirmed locally");
                self.set_editor_interaction_state(InteractionState::Editable, ctx);
            }
            ActiveNotebookDataEvent::EditRejected => {
                log::info!("Edit rejected, switching to view mode");
                self.switch_to_view(ctx);
            }
            ActiveNotebookDataEvent::BreadcrumbsChanged => {
                self.update_breadcrumbs(ctx);
            }
            ActiveNotebookDataEvent::CreatedInObjectStore => {
                ctx.emit(NotebookEvent::Pane(PaneEvent::AppStateChanged));
            }
            ActiveNotebookDataEvent::TrashStatusChanged
            | ActiveNotebookDataEvent::MovedInLocalDrive => {
                self.pane_configuration.update(ctx, |pane_config, ctx| {
                    pane_config.refresh_pane_header_overflow_menu_items(ctx)
                });
            }
        }
        ctx.notify();
    }

    /// Handle a change to the [`ActiveNotebookData`] model for this notebook.
    fn handle_active_notebook_change(
        &mut self,
        _handle: ModelHandle<ActiveNotebookData>,
        ctx: &mut ViewContext<Self>,
    ) {
        // Refresh the overflow menu to show actions that only apply to persisted notebooks.
        self.pane_configuration.update(ctx, |pane_config, ctx| {
            pane_config.refresh_pane_header_overflow_menu_items(ctx)
        });
        ctx.notify();
    }

    fn handle_appearance_change(&mut self, ctx: &mut ViewContext<Self>) {
        let appearance = Appearance::as_ref(ctx);
        let font_settings = FontSettings::as_ref(ctx);
        let new_font_size = styles::title_font_size(font_settings);
        let new_text_colors = title_text_colors(appearance);
        self.title.update(ctx, move |title_editor, ctx| {
            title_editor.set_font_size(new_font_size, ctx);
            title_editor.set_text_colors(new_text_colors, ctx);
        });
    }

    /// Handle any events emitted from the title editor view.
    fn handle_title_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Activate => {
                self.last_focused_component = FocusedComponent::Title;
                ctx.emit(NotebookEvent::Pane(PaneEvent::FocusSelf));
            }
            EditorEvent::Edited(edit_origin) => {
                // We only want to queue up a local title update
                // if this was a user-initiated request. We don't want to do this for
                // system edits because that could end up in an infinite loop (e.g.
                // open notebook -> system edit -> persist -> receive update -> system update -> ...).
                if matches!(
                    edit_origin,
                    EditOrigin::UserTyped | EditOrigin::UserInitiated
                ) {
                    self.enqueue_title_update();
                }

                let title = self.title(ctx);
                self.pane_configuration
                    .update(ctx, |pane_configuration, ctx| {
                        pane_configuration.set_title(title, ctx)
                    });
            }
            EditorEvent::Enter
            | EditorEvent::CmdEnter
            | EditorEvent::Navigate(NavigationKey::Tab) => {
                self.grab_edit_access_or_display_access_dialog(ctx);
            }
            EditorEvent::Blurred => {
                self.title.update(ctx, move |title_editor, ctx| {
                    title_editor.clear_selections(ctx);
                    ctx.notify();
                });
            }
            _ => (),
        }
    }

    /// Handle an event from the [`GrabEditAccessModal`]. This lets users steal edit access from
    /// other users.
    fn handle_grab_edit_access_modal_event(
        &mut self,
        event: &GrabEditAccessModalEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            GrabEditAccessModalEvent::Close => {
                self.active_notebook_data
                    .update(ctx, |active_notebook_data, ctx| {
                        active_notebook_data.show_grab_edit_access_modal = false;
                        ctx.notify();
                    });
            }
            GrabEditAccessModalEvent::GrabEditAccess => {
                self.active_notebook_data
                    .update(ctx, |active_notebook_data, ctx| {
                        active_notebook_data.show_grab_edit_access_modal = false;
                        ctx.notify();
                    });
                log::info!("Explicitly grabbing edit access, stealing from active editor");
                self.grab_edit_access(false, ctx);
            }
        }
        ctx.notify();
    }

    /// Reload an updated notebook.
    fn handle_notebook_updated(&mut self, notebook: &NotebookObject, ctx: &mut ViewContext<Self>) {
        self.set_title(&notebook.model().title, ctx);
        self.input.update(ctx, |input_editor, ctx| {
            input_editor.system_clear_buffer(ctx);
            input_editor.reset_with_markdown(notebook.model().data.as_str(), ctx);
        });
        ctx.notify();
    }

    /// Given a local object ID, check if it's the ID of the active notebook.
    ///
    /// This is a helper for handling [`ObjectStoreEvent`]s, which should be ignored if they're not
    /// for the active notebook.
    fn as_active_notebook_id(
        &self,
        id: &ObjectTypeAndId,
        ctx: &mut ViewContext<Self>,
    ) -> Option<ObjectStoreId> {
        id.as_notebook_id().filter(|id| {
            self.active_notebook_data
                .as_ref(ctx)
                .is_active_notebook(*id)
        })
    }

    fn handle_object_store_event(&mut self, event: &ObjectStoreEvent, ctx: &mut ViewContext<Self>) {
        match event {
            ObjectStoreEvent::ObjectUpdated {
                type_and_id,
                source: UpdateSource::External,
            } => {
                if let Some(updated_notebook) = self
                    .as_active_notebook_id(type_and_id, ctx)
                    .and_then(|notebook_id| {
                        ObjectStoreModel::as_ref(ctx).get_notebook(&notebook_id)
                    })
                    .cloned()
                {
                    self.handle_notebook_updated(&updated_notebook, ctx);
                }
            }
            ObjectStoreEvent::ObjectTrashed { .. } | ObjectStoreEvent::ObjectDeleted { .. } => {
                // Check is_trashed rather than the event ID, since this notebook could have been
                // indirectly trashed.
                if !self
                    .active_notebook_data
                    .as_ref(ctx)
                    .trash_status(ctx)
                    .is_editable()
                {
                    self.give_up_edit_access_and_start_viewing(ctx)
                }
            }
            ObjectStoreEvent::ObjectUntrashed { .. } => {
                // Re-render if this notebook was potentially untrashed. See the ObjectTrashed case
                // for why we can't rely on the event ID.
                if self
                    .active_notebook_data
                    .as_ref(ctx)
                    .trash_status(ctx)
                    .is_editable()
                {
                    ctx.notify();
                }
            }
            ObjectStoreEvent::ObjectMoved { type_and_id, .. } => {
                if self.as_active_notebook_id(type_and_id, ctx).is_some() {
                    if let Some(space) = self.active_notebook_data.as_ref(ctx).space(ctx) {
                        self.input
                            .update(ctx, |editor, ctx| editor.set_space(space, ctx));
                    }
                }
            }
            ObjectStoreEvent::ObjectCreated { type_and_id, .. } => {
                if self.as_active_notebook_id(type_and_id, ctx).is_some() {
                    // Re-render to update the status bar.
                    ctx.notify();
                }
            }
            _ => (),
        }
    }

    /// The current Markdown content of this notebook.
    pub fn content(&self, ctx: &AppContext) -> String {
        self.input.as_ref(ctx).markdown(ctx)
    }

    /// Saves the notebook's current Markdown content, via the [`UpdateManager`].
    fn save_content(&mut self, ctx: &mut ViewContext<Self>) {
        let content = Arc::new(self.content(ctx));

        // Block saving if secrets are detected in the notebook when secret redaction is enabled.
        let secret_redaction = get_secret_obfuscation_mode(ctx);
        if secret_redaction.should_redact_secret() {
            let content_escaped = ESCAPE_PUNCTUATION_REGEX
                .replace_all(&content, "$1")
                .to_string();
            let content_secrets = find_secrets_in_text(&content_escaped);
            if !content_secrets.is_empty() {
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast(
                        DismissibleToast::error(
                            "This notebook cannot be saved because its content contains secrets"
                                .to_string(),
                        ),
                        window_id,
                        ctx,
                    );
                });
                return;
            }
        }

        let active_notebook = self.active_notebook_data.as_ref(ctx).active_notebook();
        match active_notebook {
            // If the notebook has already been committed, then update local data
            // via update manager.
            ActiveNotebook::CommittedNotebook(id) => UpdateManager::handle(ctx)
                .update(ctx, move |update_manager, ctx| {
                    update_manager.update_notebook_data(content, id, ctx)
                }),
            // If the notebook hasn't been committed yet, create the notebook through update
            // manager, and update the active notebook
            ActiveNotebook::NewNotebook(notebook) => {
                if let Some(client_id) = notebook.id.into_client() {
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        update_manager.create_notebook(
                            client_id,
                            notebook.permissions.owner,
                            notebook.metadata.folder_id,
                            NotebookObjectModel {
                                title: notebook.model().title.clone(),
                                data: content.to_string(),
                                ai_document_id: notebook.model().ai_document_id,
                                conversation_id: notebook.model().conversation_id.clone(),
                            },
                            StoredObjectEventEntrypoint::Unknown,
                            true,
                            ctx,
                        );
                    });

                    self.active_notebook_data.update(ctx, |data, _| {
                        data.active_notebook =
                            ActiveNotebook::CommittedNotebook(ObjectStoreId::ClientId(client_id))
                    });
                }
            }
            ActiveNotebook::None => log::error!("Tried to save notebook, but none were active"),
        }
    }

    /// Checks if the user is the current known editor of the notebook. If they
    /// are, sets the current editor to None in local object metadata.
    fn try_give_up_edit_access(&self, ctx: &mut ViewContext<Self>) {
        let id = self.active_notebook_data.as_ref(ctx).id();
        UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
            if let Some(id) = id {
                update_manager.give_up_notebook_edit_access(id, ctx);
            }
        });
        ctx.notify();
    }

    fn give_up_edit_access_and_start_viewing(&mut self, ctx: &mut ViewContext<Self>) {
        self.try_give_up_edit_access(ctx);
        self.switch_to_view(ctx);
    }

    /// Save any changes to the notebook.
    fn handle_save(&mut self, _: NotebookUpdateRequestDebounceArg, ctx: &mut ViewContext<Self>) {
        if self.content_is_dirty {
            self.save_content(ctx);
            self.content_is_dirty = false;
        }

        if self.title_is_dirty {
            self.persist_title_to_local_store(ctx);
            self.title_is_dirty = false;
        }
    }

    /// Enqueue a save of the notebook's content.
    fn enqueue_content_update(&mut self, ctx: &mut ViewContext<Self>) {
        self.content_is_dirty = true;
        report_if_error!(self
            .save_tx
            .try_send(NotebookUpdateRequestDebounceArg {})
            .context("Error enqueuing content save"));
        self.active_notebook_data.update(ctx, |data, ctx| {
            // Mark the notebook as saving as soon as there are changes to be saved. It won't be
            // marked as Saved until the local object-store update completes.
            data.saving_status = SavingStatus::Saving;
            ctx.notify();
        });
        ctx.notify();
    }

    /// Enqueue a save of the notebook's title.
    fn enqueue_title_update(&mut self) {
        self.title_is_dirty = true;
        report_if_error!(self
            .save_tx
            .try_send(NotebookUpdateRequestDebounceArg {})
            .context("Error enqueuing title save"));
    }

    fn handle_input_editor_event(&mut self, event: &EditorViewEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorViewEvent::Edited => {
                self.enqueue_content_update(ctx);
            }
            EditorViewEvent::Focused => {
                self.last_focused_component = FocusedComponent::Input;
                ctx.emit(NotebookEvent::Pane(PaneEvent::FocusSelf));
            }
            EditorViewEvent::Navigate(NavigationKey::ShiftTab) => {
                // Focus the title editor, but do not give up local edit access.
                ctx.focus(&self.title);
            }
            EditorViewEvent::Navigate(_) => (),
            EditorViewEvent::RunWorkflow(workflow) => self.run_notebook_workflow(workflow, ctx),
            EditorViewEvent::EditWorkflow(workflow_id) => {
                ctx.emit(NotebookEvent::EditWorkflow(*workflow_id))
            }
            EditorViewEvent::OpenedBlockInsertionMenu(_)
            | EditorViewEvent::OpenedEmbeddedObjectSearch
            | EditorViewEvent::OpenedFindBar
            | EditorViewEvent::InsertedEmbeddedObject(_)
            | EditorViewEvent::CopiedBlock { .. }
            | EditorViewEvent::NavigatedCommands
            | EditorViewEvent::ChangedSelectionMode(_)
            | EditorViewEvent::OpenFile { .. }
            | EditorViewEvent::CmdEnter
            | EditorViewEvent::EscapePressed
            | EditorViewEvent::TextSelectionChanged => (),
        }
    }

    fn switch_to_view(&mut self, ctx: &mut ViewContext<Self>) {
        self.active_notebook_data.update(ctx, |data, ctx| {
            data.mode = Mode::View;
            ctx.notify();
        });
        self.set_editor_interaction_state(InteractionState::Selectable, ctx);
        ctx.notify();
    }

    pub fn is_plan(&self, ctx: &AppContext) -> bool {
        self.active_notebook_data
            .as_ref(ctx)
            .ai_document_id(ctx)
            .is_some()
    }

    fn mode<C: ModelAsRef>(&self, ctx: &C) -> Mode {
        self.active_notebook_data.as_ref(ctx).mode
    }

    fn mode_app_ctx(&self, ctx: &AppContext) -> Mode {
        self.active_notebook_data.as_ref(ctx).mode
    }

    /// The ID of the notebook open in this view.
    pub fn notebook_id(&self, ctx: &impl ModelAsRef) -> Option<ObjectStoreId> {
        self.active_notebook_data.as_ref(ctx).id()
    }

    /// The stable object ID of this notebook, if it has one.
    fn legacy_object_store_notebook_id(&self, ctx: &ViewContext<Self>) -> Option<NotebookId> {
        self.notebook_id(ctx)?.into_stable().map(Into::into)
    }

    /// Puts the nodebook into edit mode and focuses the editor. The caller is responsible for
    /// checking that the notebook is editable.
    fn switch_to_edit(&mut self, ctx: &mut ViewContext<Self>) {
        self.active_notebook_data.update(ctx, |data, ctx| {
            data.mode = Mode::Editing;
            ctx.notify();
        });

        self.set_editor_interaction_state(InteractionState::Editable, ctx);
    }

    /// Grabs local notebook edit access.
    fn grab_edit_access(&mut self, optimistically_grant_access: bool, ctx: &mut ViewContext<Self>) {
        let active_notebook = self.active_notebook_data.as_ref(ctx);
        if !active_notebook.trash_status(ctx).is_editable() {
            // Do not allow grabbing edit access if the notebook is trashed or feature flag is turned off.
            return;
        }
        let id = active_notebook.id();
        UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
            if let Some(id) = id {
                update_manager.grab_notebook_edit_access(id, optimistically_grant_access, ctx);
            }
        });

        // If we are optimistically granting access, go ahead and switch into edit mode.
        if optimistically_grant_access {
            self.switch_to_edit(ctx);
        }

        ctx.focus(&self.input);
        ctx.notify();
    }

    /// Called when a user hits the edit button from within a notebook view.
    /// If there's not another editor, grabs notebook edit access and directly switches it
    /// into edit mode. If there is another editor currently, displays the grab edit access
    /// dialog.
    pub fn grab_edit_access_or_display_access_dialog(&mut self, ctx: &mut ViewContext<Self>) {
        let active_notebook_data = self.active_notebook_data.as_ref(ctx);
        if active_notebook_data.has_conflicts(ctx) {
            // Do not attempt to grab edit access if there are conflicts.
            return;
        }

        let current_editor = active_notebook_data
            .current_editor(ctx)
            .unwrap_or(Editor::no_editor());
        if current_editor.state == EditorState::OtherUserActive {
            self.active_notebook_data.update(ctx, |data, ctx| {
                data.show_grab_edit_access_modal = true;
                ctx.notify();
            });
        } else {
            log::info!("Explicitly grabbing edit access, no active editor");
            self.grab_edit_access(true, ctx);
        }

        self.focus_input(ctx);
        ctx.notify();
    }

    /// Reset the notebook title editor's content as a system edit, which is not persisted as object-store IDs.
    fn set_title(&mut self, notebook_title: &str, ctx: &mut ViewContext<Self>) {
        self.title.update(ctx, |title, ctx| {
            title.system_reset_buffer_text(notebook_title, ctx);
        });
    }

    fn set_content(&mut self, notebook: &NotebookObject, ctx: &mut ViewContext<Self>) {
        self.input.update(ctx, |input, ctx| {
            input.reset_with_markdown(notebook.model().data.as_str(), ctx);
        });

        self.switch_to_view(ctx);
    }

    fn increase_font_size(&mut self, ctx: &mut ViewContext<Self>) {
        report_if_error!(increase_notebook_font_size(ctx))
    }

    fn decrease_font_size(&mut self, ctx: &mut ViewContext<Self>) {
        report_if_error!(decrease_notebook_font_size(ctx))
    }

    fn apply_font_size_to_setting(&mut self, new_font_size: f32, ctx: &mut ViewContext<Self>) {
        FontSettings::handle(ctx).update(ctx, |font_settings, ctx| {
            report_if_error!(font_settings
                .notebook_font_size
                .set_value(new_font_size, ctx))
        });
    }

    fn view_in_local_drive(&mut self, id: LocalDriveItemId, ctx: &mut ViewContext<Self>) {
        ctx.emit(NotebookEvent::ViewInLocalDrive(id));
    }

    fn duplicate_object(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(notebook_id) = self.notebook_id(ctx) {
            UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                update_manager.duplicate_object(
                    &ObjectTypeAndId::from_id_and_type(notebook_id, ObjectType::Notebook),
                    ctx,
                );
            });
            ctx.notify();
        }
    }

    fn trash_object(&mut self, ctx: &mut ViewContext<Self>) {
        if let Some(notebook_id) = self.notebook_id(ctx) {
            self.close(ctx);

            UpdateManager::handle(ctx).update(ctx, move |update_manager, ctx| {
                update_manager.trash_object(
                    ObjectTypeAndId::from_id_and_type(notebook_id, ObjectType::Notebook),
                    ctx,
                );
            });
        }
    }

    fn untrash_notebook(&self, ctx: &mut ViewContext<Self>) {
        if let Some(notebook_id) = self.notebook_id(ctx) {
            UpdateManager::handle(ctx).update(ctx, move |update_manager, ctx| {
                update_manager.untrash_object(
                    ObjectTypeAndId::from_id_and_type(notebook_id, ObjectType::Notebook),
                    ctx,
                );
            });
        }
    }

    /// Start exporting this notebook.
    fn export(&self, ctx: &mut ViewContext<Self>) {
        if let Some(notebook_id) = self.notebook_id(ctx) {
            let window_id = ctx.window_id();
            ExportManager::handle(ctx).update(ctx, |export_manager, ctx| {
                export_manager.export(
                    window_id,
                    &[ObjectTypeAndId::from_id_and_type(
                        notebook_id,
                        ObjectType::Notebook,
                    )],
                    ctx,
                )
            });
        }
    }

    fn copy_notebook_contents_to_clipboard(&mut self, ctx: &mut ViewContext<Self>) {
        self.input.update(ctx, |input, ctx| {
            input.model().update(ctx, |model, ctx| model.copy_all(ctx))
        });
    }

    /// Items to show in the pane header overflow menu.
    fn overflow_menu_items(&self, ctx: &AppContext) -> Vec<MenuItem<NotebookAction>> {
        let active_notebook_data = self.active_notebook_data.as_ref(ctx);
        let mut menu_items = Vec::new();

        if active_notebook_data.trash_status(ctx) != TrashStatus::Active {
            return menu_items;
        }

        if let Some(ai_document_id) = active_notebook_data.ai_document_id(ctx) {
            menu_items.push(
                MenuItemFields::new(crate::t!("notebook-menu-attach-active-session"))
                    .with_on_select_action(NotebookAction::AttachPlanAsContext(ai_document_id))
                    .with_icon(icons::Icon::Paperclip)
                    .into_item(),
            );
        }

        menu_items.push(
            MenuItemFields::new(crate::t!("common-duplicate"))
                .with_on_select_action(NotebookAction::Duplicate)
                .with_icon(icons::Icon::Duplicate)
                .into_item(),
        );

        #[cfg(feature = "local_fs")]
        {
            menu_items.push(
                MenuItemFields::new(crate::t!("common-export"))
                    .with_on_select_action(NotebookAction::Export)
                    .with_icon(icons::Icon::Download)
                    .into_item(),
            );
        }

        menu_items.push(
            MenuItemFields::new(crate::t!("common-trash"))
                .with_on_select_action(NotebookAction::Trash)
                .with_icon(icons::Icon::Trash)
                .into_item(),
        );

        menu_items
    }

    /// Takes a given `notebook_id`, and tries to load it into view after initial load completes.
    /// If the notebook still does not exist in memory after initial load, displaces an error message in
    /// the given window.
    ///
    /// Used for code paths such as link opening, where we are often trying to open notebooks before
    /// the local object store restore has completed.
    pub fn wait_for_initial_load_then_load(
        &mut self,
        notebook_id: ObjectStoreId,
        settings: &LocalDriveObjectSettings,
        window_id: WindowId,
        ctx: &mut ViewContext<Self>,
    ) {
        let initial_load_complete =
            crate::object_store::model::persistence::ObjectStoreModel::as_ref(ctx)
                .initial_load_complete();
        // TODO @ianhodge CLD-2002: it could be nice to have a loading screen here while we wait for the load
        let settings = settings.clone();
        ctx.spawn(initial_load_complete, move |me, _, ctx| {
            let notebook = ObjectStoreModel::as_ref(ctx)
                .get_notebook(&notebook_id)
                .cloned();
            if let Some(notebook) = notebook {
                me.load(notebook, &settings, ctx);
            } else {
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast_by_type(
                        ToastType::StoredObjectNotFound,
                        window_id,
                        ctx,
                    );
                });
                log::warn!("Tried to open unknown notebook {notebook_id:?}");
            }
        });
    }

    /// Takes a `NotebookObject` and loads it into the view.
    ///
    /// Namely, we reset the title and body's undo stack and we set the buffer to be
    /// that of the object-store notebook's content.
    ///
    /// The returned [`SpawnedFutureHandle`] guards asynchronous work to claim
    /// local edit access and start editing if there is not already an editor.
    pub fn load(
        &mut self,
        notebook: NotebookObject,
        settings: &LocalDriveObjectSettings,
        ctx: &mut ViewContext<Self>,
    ) -> SpawnedFutureHandle {
        self.set_title(&notebook.model().title, ctx);
        self.set_content(&notebook, ctx);

        self.active_notebook_data.update(ctx, |data, ctx| {
            data.open_existing(notebook.id, ctx);
        });
        self.input.update(ctx, |editor, ctx| {
            // TODO(ben): This is used for filtering in the embed UI, and should also probably be
            // owner-based.
            editor.set_space(notebook.space(ctx), ctx);
        });

        // Once local metadata has loaded, check if we can eagerly edit the notebook.
        let has_metadata = crate::object_store::model::persistence::ObjectStoreModel::as_ref(ctx)
            .initial_load_complete();
        let baton_future = ctx.spawn(has_metadata, |me, _, ctx| {
            let active_notebook_data = me.active_notebook_data.as_ref(ctx);

            if active_notebook_data.has_conflicts(ctx) {
                log::debug!("Notebook has conflicts, opening in view mode");
            } else {
                let current_editor = active_notebook_data.current_editor(ctx);

                // If there's not currently an editor or the current editor has been idle, we want to automatically
                // switch the user into edit mode.
                match current_editor {
                    Some(editor) => {
                        let email = editor.email.unwrap_or_default();
                        match editor.state {
                            EditorState::None => {
                                log::info!("Optimistically grabbing edit access, no notebook editor");
                                me.grab_edit_access(true, ctx);
                            }
                            EditorState::CurrentUser => {
                                safe_info!(
                                    safe: ("Optimistically grabbing edit access, already the editor"),
                                    full: ("Optmisitically grabbing edit access, user {email} is already the editor")
                                );
                                me.grab_edit_access(true, ctx);
                            }
                            EditorState::OtherUserIdle => {
                                    safe_info!(
                                        safe: ("Optimistically grabbing edit access, editor is idle"),
                                        full: ("Optmisitically grabbing edit access, editor {email} is idle")
                                    );
                                    me.grab_edit_access(true, ctx);
                                }
                            EditorState::OtherUserActive => {
                                log::info!("Opening in view mode, notebook is being edited")
                            }
                        }
                    }
                    None => {
                        log::info!("Opening in view mode, unknown editor");
                    }
                }
            }
        });
        self.update_breadcrumbs(ctx);
        // Ashide Phase 2a: invitee-driven sharing dialog removed.
        if let Some(focused_folder_id) = settings.focused_folder_id.map(ObjectStoreId::StableId) {
            self.view_in_local_drive(
                LocalDriveItemId::Object(ObjectTypeAndId::Folder(focused_folder_id)),
                ctx,
            );
        }

        ctx.notify();
        baton_future
    }

    /// Reset this view to show a new, empty notebook.
    pub fn open_new_notebook(
        &mut self,
        title: Option<String>,
        owner: Owner,
        initial_folder_id: Option<ObjectStoreId>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.active_notebook_data.update(ctx, |data, ctx| {
            data.open_new(owner, initial_folder_id, ctx);
        });
        self.input.update(ctx, |input_editor, ctx| {
            input_editor.system_clear_buffer(ctx);
            let space = UserWorkspaces::as_ref(ctx).owner_to_space(owner, ctx);
            input_editor.set_space(space, ctx);
        });

        if let Some(title) = title {
            self.set_title(&title, ctx);
            self.persist_title_to_local_store(ctx);
        } else {
            self.title.update(ctx, |title_editor, ctx| {
                title_editor.system_clear_buffer(true, ctx);
            });
        }

        self.update_breadcrumbs(ctx);

        self.switch_to_edit(ctx);
    }

    /// Persists the notebook title to the local object store with the current contents of the title editor.
    pub fn persist_title_to_local_store(&mut self, ctx: &mut ViewContext<Self>) {
        let title: Arc<String> = self.title.as_ref(ctx).buffer_text(ctx).into();

        // Block saving if secrets are detected in the notebook title when secret redaction is enabled.
        let secret_redaction = get_secret_obfuscation_mode(ctx);
        if secret_redaction.should_redact_secret() {
            let title_escaped = ESCAPE_PUNCTUATION_REGEX
                .replace_all(&title, "$1")
                .to_string();
            let title_secrets = find_secrets_in_text(&title_escaped);
            if !title_secrets.is_empty() {
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                    toast_stack.add_ephemeral_toast(
                        DismissibleToast::error(
                            "This notebook cannot be saved because its title contains secrets"
                                .to_string(),
                        ),
                        window_id,
                        ctx,
                    );
                });
                return;
            }
        }

        let active_notebook = self.active_notebook_data.as_ref(ctx).active_notebook();
        match active_notebook {
            // If the notebook has already been committed, update local object-store state.
            ActiveNotebook::CommittedNotebook(id) => UpdateManager::handle(ctx)
                .update(ctx, |update_manager, ctx| {
                    update_manager.update_notebook_title(title.clone(), id, ctx)
                }),
            // If the notebook hasn't been committed yet, create the notebook through update
            // manager, and update the active notebook
            ActiveNotebook::NewNotebook(notebook) => {
                if let Some(client_id) = notebook.id.into_client() {
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        update_manager.create_notebook(
                            client_id,
                            notebook.permissions.owner,
                            notebook.metadata.folder_id,
                            NotebookObjectModel {
                                title: title.to_string(),
                                data: notebook.model().data.to_owned(),
                                ai_document_id: notebook.model().ai_document_id,
                                conversation_id: notebook.model().conversation_id.clone(),
                            },
                            StoredObjectEventEntrypoint::Unknown,
                            true,
                            ctx,
                        );
                    });
                    self.active_notebook_data.update(ctx, |data, _| {
                        data.active_notebook =
                            ActiveNotebook::CommittedNotebook(ObjectStoreId::ClientId(client_id))
                    });
                }
            }
            ActiveNotebook::None => log::error!("Tried to save notebook, but none were active"),
        }
    }

    /// Update the breadcrumbs for this notebook.
    fn update_breadcrumbs(&mut self, ctx: &mut ViewContext<Self>) {
        self.details_bar
            .update_breadcrumbs(self.active_notebook_data.as_ref(ctx), ctx);
        ctx.notify();
    }

    /// Save this notebook and give up edit access before detaching it from a pane.
    pub fn on_detach(&mut self, ctx: &mut ViewContext<Self>) {
        // If there are un-saved edits, persist them now, since the asynchronous update callback
        // is unlikely to run again.
        self.handle_save(NotebookUpdateRequestDebounceArg {}, ctx);

        // Give up local notebook edit access on quitting.
        self.try_give_up_edit_access(ctx);
    }

    pub fn toggle_mode(&mut self, ctx: &mut ViewContext<Self>) {
        match self.mode(ctx) {
            Mode::Editing => {
                self.give_up_edit_access_and_start_viewing(ctx);
            }
            Mode::View => self.grab_edit_access_or_display_access_dialog(ctx),
        }
    }

    fn run_notebook_workflow(&self, workflow: &NotebookWorkflow, ctx: &mut ViewContext<Self>) {
        // If the notebook workflow was anonymous, synthesize metadata for it.
        let workflow_type =
            workflow.named_workflow(|| Some(format!("Command from {}", self.title(ctx))));

        let notebook_id = self.legacy_object_store_notebook_id(ctx);
        let source = workflow.source.unwrap_or_else(|| {
            let owner = self.active_notebook_data.as_ref(ctx).owner(ctx);
            WorkflowSource::Notebook {
                notebook_id,
                location: owner
                    .map(Into::into)
                    .unwrap_or(NotebookLocation::PersonalDrive),
            }
        });

        ctx.emit(NotebookEvent::RunWorkflow {
            workflow: workflow_type,
            source,
        });
        ctx.notify();
    }

    fn conflict_dialog_refresh_button_clicked(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(id) = self.notebook_id(ctx) else {
            return;
        };

        UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
            update_manager.replace_object_with_conflict(&id.uid(), ctx);
        });

        // Reload the notebook now that the local object store has been updated.
        // This will also switch back to edit mode if there isn't an active editor.
        if let Some(notebook) = ObjectStoreModel::as_ref(ctx).get_notebook(&id) {
            self.load(notebook.clone(), &LocalDriveObjectSettings::default(), ctx);
        }
        ctx.notify();
    }

    fn render_body(&self) -> Box<dyn Element> {
        let editor = self.input.clone();
        let saved_position = self.view_position_id.clone();

        EventHandler::new(styles::wrap_body(ChildView::new(&self.input).finish()))
            .on_right_mouse_down(move |ctx, _, position| {
                show_rich_editor_context_menu::<NotebookAction>(
                    ctx,
                    position,
                    &saved_position,
                    &editor,
                );
                DispatchEventResult::StopPropagation
            })
            .finish()
    }

    fn render_title(&self, app: &AppContext) -> Box<dyn Element> {
        let title_editor = self.title.clone();
        let saved_position = self.view_position_id.clone();
        let appearance = Appearance::as_ref(app);
        let title = EventHandler::new(Clipped::new(ChildView::new(&self.title).finish()).finish())
            .on_right_mouse_down(move |ctx, _, position| {
                show_text_editor_context_menu::<NotebookAction>(
                    ctx,
                    position,
                    &saved_position,
                    &title_editor,
                );
                DispatchEventResult::StopPropagation
            })
            .finish();

        let active_notebook_data = self.active_notebook_data.as_ref(app);

        let details = if active_notebook_data.trash_status(app).is_editable() {
            Some(
                self.details_bar
                    .render(active_notebook_data, appearance, app),
            )
        } else {
            None
        };

        styles::wrap_title(title, details)
    }

    fn render_trash_banner(&self, app: &AppContext) -> Option<Box<dyn Element>> {
        let deleted = match self.active_notebook_data.as_ref(app).trash_status(app) {
            TrashStatus::Active => return None,
            TrashStatus::Trashed => false,
            TrashStatus::Deleted => true,
        };
        let appearance = Appearance::as_ref(app);

        let mut stack = Stack::new();

        let text = if deleted {
            "You no longer have access to this notebook"
        } else {
            "Notebook was moved to trash"
        };
        stack.add_child(
            Align::new(
                Flex::row()
                    .with_children([
                        ConstrainedBox::new(
                            icons::Icon::Trash
                                .to_warpui_icon(appearance.theme().foreground())
                                .finish(),
                        )
                        .with_width(16.)
                        .with_height(16.)
                        .finish(),
                        appearance
                            .ui_builder()
                            .span(text)
                            .with_style(UiComponentStyles {
                                font_size: Some(appearance.ui_font_subheading()),
                                ..Default::default()
                            })
                            .build()
                            .with_padding_left(8.)
                            .finish(),
                    ])
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_main_axis_alignment(MainAxisAlignment::Center)
                    .with_main_axis_size(MainAxisSize::Max)
                    .finish(),
            )
            .finish(),
        );

        let action_row = if deleted {
            Shrinkable::new(1., Empty::new().finish()).finish()
        } else {
            let mut action_row = Flex::row()
                .with_main_axis_alignment(MainAxisAlignment::End)
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);

            let ui_builder = appearance.ui_builder().clone();
            action_row.add_child(
                Align::new(
                    appearance
                        .ui_builder()
                        .button(
                            ButtonVariant::Basic,
                            self.button_mouse_states.restore_from_trash_button.clone(),
                        )
                        .with_tooltip(move || {
                            ui_builder
                                .tool_tip(crate::t!("notebook-tooltip-restore-from-trash"))
                                .build()
                                .finish()
                        })
                        .with_text_label(crate::t!("common-restore"))
                        .build()
                        .on_click(|ctx, _, _| ctx.dispatch_typed_action(NotebookAction::Untrash))
                        .finish(),
                )
                .finish(),
            );
            action_row.finish()
        };

        stack.add_child(Align::new(action_row).right().finish());

        Some(
            Container::new(
                ConstrainedBox::new(stack.finish())
                    .with_min_height(40.)
                    .finish(),
            )
            .with_horizontal_padding(16.)
            .with_background(appearance.theme().surface_2())
            .finish(),
        )
    }

    fn render_local_save_banner(
        &self,
        save_notice: NotebookLocalSaveNotice,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let banner = Shrinkable::new(
            1.,
            appearance
                .ui_builder()
                .wrappable_text(
                    match save_notice {
                        NotebookLocalSaveNotice::FeatureNotAvailable => {
                            crate::t!("notebook-local-save-feature-not-available-message")
                        }
                        NotebookLocalSaveNotice::InConflict => {
                            crate::t!("notebook-local-save-conflict-resolution-message")
                        }
                    },
                    true,
                )
                .with_style(UiComponentStyles {
                    font_size: Some(appearance.ui_font_subheading()),
                    ..Default::default()
                })
                .build()
                .with_margin_bottom(BANNER_VERTICAL_MARGIN)
                .with_margin_top(BANNER_VERTICAL_MARGIN)
                .with_margin_right(HEADER_MARGIN)
                .with_margin_left(HEADER_MARGIN)
                .finish(),
        )
        .finish();

        let mut action_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        let ui_builder = appearance.ui_builder().clone();
        action_row.add_child(
            Container::new(
                Align::new(
                    appearance
                        .ui_builder()
                        .button(
                            ButtonVariant::Basic,
                            self.button_mouse_states
                                .conflict_resolution_copy_all_button
                                .clone(),
                        )
                        .with_tooltip(move || {
                            ui_builder
                                .tool_tip(crate::t!("notebook-tooltip-copy-to-clipboard"))
                                .build()
                                .finish()
                        })
                        .with_text_label(crate::t!("notebook-copy-all"))
                        .build()
                        .on_click(|ctx, _, _| {
                            ctx.dispatch_typed_action(NotebookAction::CopyToClipboard)
                        })
                        .finish(),
                )
                .finish(),
            )
            .with_margin_bottom(BANNER_VERTICAL_MARGIN)
            .with_margin_right(HEADER_MARGIN)
            .with_margin_left(HEADER_MARGIN)
            .finish(),
        );

        if matches!(save_notice, NotebookLocalSaveNotice::InConflict) {
            let ui_builder = appearance.ui_builder().clone();
            action_row.add_child(
                Container::new(
                    Align::new(
                        appearance
                            .ui_builder()
                            .button(
                                ButtonVariant::Basic,
                                self.button_mouse_states
                                    .conflict_resolution_refresh_button
                                    .clone(),
                            )
                            .with_tooltip(move || {
                                ui_builder
                                    .tool_tip(crate::t!("notebook-refresh-notebook"))
                                    .build()
                                    .finish()
                            })
                            .with_text_label(crate::t!("common-refresh"))
                            .build()
                            .on_click(|ctx, _, _| {
                                ctx.dispatch_typed_action(
                                    NotebookAction::ConflictResolutionBannerRefreshClicked,
                                )
                            })
                            .finish(),
                    )
                    .finish(),
                )
                .with_margin_bottom(BANNER_VERTICAL_MARGIN)
                .with_margin_right(HEADER_MARGIN)
                .finish(),
            );
        }

        Container::new(
            Flex::column()
                .with_children([banner, action_row.finish()])
                .finish(),
        )
        .with_horizontal_padding(16.)
        .with_background(appearance.theme().surface_2())
        .finish()
    }
}

impl Entity for NotebookView {
    type Event = NotebookEvent;
}

impl View for NotebookView {
    fn ui_name() -> &'static str {
        "NotebookView"
    }

    fn accessibility_contents(&self, ctx: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new_without_help(
            format!("{} notebook", self.title(ctx)),
            WarpA11yRole::TextRole,
        ))
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focused = true;
            ctx.notify();
        }
    }

    fn on_blur(&mut self, blur_ctx: &BlurContext, ctx: &mut ViewContext<Self>) {
        if blur_ctx.is_self_blurred() {
            self.focused = false;
            ctx.notify();
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn warpui::Element> {
        let mut content = Flex::column();
        content.extend(self.render_trash_banner(app));
        content.add_child(self.render_title(app));
        content.add_child(Shrinkable::new(1., self.render_body()).finish());

        let notebook = Align::new(content.finish()).top_left().finish();

        let mut stack = Stack::new();

        match self.mode_app_ctx(app) {
            // For editing mode, there is currently no use-case for focusing the notebook
            // view itself when clicking outside of the editor. We could change this behavior
            // if we need to in the future.
            Mode::Editing => stack.add_child(notebook),
            Mode::View => stack.add_child(
                EventHandler::new(notebook)
                    .on_left_mouse_down(|ctx, _, _| {
                        ctx.dispatch_typed_action(NotebookAction::Focus);
                        DispatchEventResult::StopPropagation
                    })
                    .finish(),
            ),
        };

        if self
            .active_notebook_data
            .as_ref(app)
            .show_grab_edit_access_modal
        {
            stack.add_child(ChildView::new(&self.grab_edit_access_modal).finish());
        }

        if self
            .active_notebook_data
            .as_ref(app)
            .feature_not_available()
        {
            stack.add_child(self.render_local_save_banner(
                NotebookLocalSaveNotice::FeatureNotAvailable,
                Appearance::as_ref(app),
            ));
        } else if self.active_notebook_data.as_ref(app).has_conflicts(app) {
            stack.add_child(self.render_local_save_banner(
                NotebookLocalSaveNotice::InConflict,
                Appearance::as_ref(app),
            ));
        }

        self.context_menu.render(&mut stack);

        SavePosition::new(stack.finish(), &self.view_position_id).finish()
    }

    fn keymap_context(&self, app: &AppContext) -> warpui::keymap::Context {
        let mut context = Self::default_keymap_context();

        match self.mode_app_ctx(app) {
            Mode::Editing => context.set.insert("NotebookEditing"),
            Mode::View => context.set.insert("NotebookViewing"),
        };

        context.set.insert("NotebookIsEditable");

        let font_settings = FontSettings::as_ref(app);
        if !font_settings.match_notebook_to_monospace_font_size.value() {
            context.set.insert("NotMatchNotebookToMonospaceSize");
        }

        context
    }
}

/// Colors to use for the title editor.
fn title_text_colors(appearance: &Appearance) -> TextColors {
    TextColors {
        default_color: styles::title_text_fill(appearance),
        ..TextColors::from_appearance(appearance)
    }
}

impl TypedActionView for NotebookView {
    type Action = NotebookAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            NotebookAction::Focus => ctx.focus_self(),
            NotebookAction::ToggleMode => self.toggle_mode(ctx),
            NotebookAction::Close => ctx.emit(NotebookEvent::Pane(PaneEvent::Close)),
            NotebookAction::ConflictResolutionBannerRefreshClicked => {
                self.conflict_dialog_refresh_button_clicked(ctx)
            }
            NotebookAction::IncreaseFontSize => self.increase_font_size(ctx),
            NotebookAction::DecreaseFontSize => self.decrease_font_size(ctx),
            NotebookAction::ResetFontSize => {
                self.apply_font_size_to_setting(NotebookFontSize::default_value(), ctx)
            }
            NotebookAction::ViewInLocalDrive(id) => self.view_in_local_drive(*id, ctx),
            NotebookAction::FocusTerminalInput => {
                ctx.emit(NotebookEvent::Pane(PaneEvent::FocusActiveSession))
            }
            NotebookAction::ContextMenu(action) => {
                self.context_menu.handle_action(action, ctx);
            }
            NotebookAction::Duplicate => self.duplicate_object(ctx),
            NotebookAction::Trash => self.trash_object(ctx),
            NotebookAction::Untrash => self.untrash_notebook(ctx),
            NotebookAction::CopyToClipboard => self.copy_notebook_contents_to_clipboard(ctx),
            NotebookAction::Export => self.export(ctx),
            NotebookAction::AttachPlanAsContext(id) => {
                ctx.emit(NotebookEvent::AttachPlanAsContext(*id))
            }
        };
    }
}

impl BackingView for NotebookView {
    type PaneHeaderOverflowMenuAction = NotebookAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn pane_header_overflow_menu_items(&self, ctx: &AppContext) -> Vec<MenuItem<NotebookAction>> {
        self.overflow_menu_items(ctx)
    }

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut ViewContext<Self>,
    ) {
        self.handle_action(action, ctx);
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        ctx.emit(NotebookEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext,
        app: &AppContext,
    ) -> view::HeaderContent {
        view::HeaderContent::simple(self.pane_configuration.as_ref(app).title())
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, ctx: &mut ViewContext<Self>) {
        ctx.subscribe_to_model(
            focus_handle.focus_state_handle(),
            |notebook, _handle, event, ctx| {
                notebook.handle_focus_state_event(event, ctx);
            },
        );

        self.focus_handle = Some(focus_handle.clone());
        self.context_menu.set_focus_handle(focus_handle);
    }
}

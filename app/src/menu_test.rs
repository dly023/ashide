use super::{Event, Menu, MenuAction, MenuItem, MenuItemFields, SelectAction, SubMenu};

use warp_core::ui::appearance::Appearance;
use warpui::{
    elements::ChildView, platform::WindowStyle, App, Element, Entity, TypedActionView, View,
    ViewContext, ViewHandle,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum TestAction {
    Root,
    ChildOne,
    ChildTwo,
}

struct MenuRebuildingParent {
    menu: ViewHandle<Menu<TestAction>>,
    handled: bool,
    closed: bool,
    action_observed_closed: bool,
}

impl MenuRebuildingParent {
    fn new(ctx: &mut ViewContext<Self>) -> Self {
        let menu = ctx.add_typed_action_view(|ctx| {
            let mut menu = Menu::new();
            menu.set_items(
                vec![MenuItemFields::new("root")
                    .with_on_select_action(TestAction::Root)
                    .into_item()],
                ctx,
            );
            menu
        });
        let parent = Self {
            menu,
            handled: false,
            closed: false,
            action_observed_closed: false,
        };
        ctx.subscribe_to_view(&parent.menu, |parent, _, event, _| {
            if matches!(
                event,
                Event::Close {
                    via_select_item: true
                }
            ) {
                parent.closed = true;
            }
        });
        parent
    }
}

impl Entity for MenuRebuildingParent {
    type Event = ();
}

impl View for MenuRebuildingParent {
    fn ui_name() -> &'static str {
        "MenuRebuildingParent"
    }

    fn render(&self, _app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        ChildView::new(&self.menu).finish()
    }
}

impl TypedActionView for MenuRebuildingParent {
    type Action = TestAction;

    fn handle_action(&mut self, action: &TestAction, ctx: &mut ViewContext<Self>) {
        if action == &TestAction::Root {
            self.handled = true;
            self.action_observed_closed = self.closed;
            self.menu.update(ctx, |menu, ctx| {
                menu.set_items(vec![MenuItemFields::new("rebuilt").into_item()], ctx);
            });
        }
    }
}

fn test_submenu_items() -> Vec<MenuItem<TestAction>> {
    vec![
        MenuItem::Submenu {
            fields: MenuItemFields::new_submenu("submenu"),
            menu: SubMenu::new(vec![
                MenuItemFields::new("child one")
                    .with_on_select_action(TestAction::ChildOne)
                    .into_item(),
                MenuItemFields::new("child two")
                    .with_on_select_action(TestAction::ChildTwo)
                    .into_item(),
            ]),
        },
        MenuItemFields::new("root")
            .with_on_select_action(TestAction::Root)
            .into_item(),
    ]
}

fn two_submenu_items() -> Vec<MenuItem<TestAction>> {
    vec![
        MenuItem::Submenu {
            fields: MenuItemFields::new_submenu("first submenu"),
            menu: SubMenu::new(vec![MenuItemFields::new("first child")
                .with_on_select_action(TestAction::ChildOne)
                .into_item()]),
        },
        MenuItem::Submenu {
            fields: MenuItemFields::new_submenu("second submenu"),
            menu: SubMenu::new(vec![MenuItemFields::new("second child")
                .with_on_select_action(TestAction::ChildTwo)
                .into_item()]),
        },
    ]
}

#[test]
fn test_menu_item_selectable() {
    assert!(MenuItemFields::<()>::new("normal").into_item().selectable());
    assert!(!MenuItemFields::<()>::new("disabled")
        .with_disabled(true)
        .into_item()
        .selectable());
    assert!(!MenuItem::<()>::Separator.selectable());
}

#[test]
fn test_next_and_previous_indexes() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let items = vec![
            MenuItemFields::<()>::new("item1")
                .with_disabled(true)
                .into_item(),
            MenuItemFields::<()>::new("item2").into_item(),
            MenuItemFields::<()>::new("item3")
                .with_disabled(true)
                .into_item(),
            MenuItemFields::<()>::new("item4").into_item(),
            MenuItemFields::<()>::new("item5").into_item(),
        ];

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<()>::new();
            menu.set_items(items, ctx);
            menu
        });

        menu.update(&mut app, |menu, _ctx| {
            assert!(menu.selected_item().is_none());

            menu.menu
                .select_internal(SelectAction::Index { row: 1, item: 0 });
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item2"
            );

            // Make sure we skip the disabled menu items
            menu.menu.select_internal(SelectAction::Next);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item4"
            );

            menu.menu.select_internal(SelectAction::Next);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item5"
            );

            // Make sure we go around
            menu.menu.select_internal(SelectAction::Next);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item2"
            );

            // Makre sure we go around with Prev action too
            menu.menu.select_internal(SelectAction::Previous);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item5"
            );

            menu.menu.select_internal(SelectAction::Previous);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item4"
            );

            // Makre sure we skip the disabled ones for previous as well
            menu.menu.select_internal(SelectAction::Previous);
            assert!(menu.selected_item().is_some());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "item2"
            );
        });
    })
}

#[test]
fn test_right_opens_selected_submenu_and_selects_first_child() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.set_selected_by_index(0, ctx);
            menu.handle_action(&MenuAction::OpenSubmenu, ctx);

            assert_eq!(menu.selected_index(), Some(0));
            let submenu = menu.menu.selected_submenu().unwrap();
            assert_eq!(submenu.selected_index(), Some(0));
            assert_eq!(
                submenu.selected_item().unwrap().fields().unwrap().label(),
                "child one"
            );
        });
    })
}

#[test]
fn test_up_and_down_navigate_the_active_submenu() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.set_selected_by_index(0, ctx);
            menu.handle_action(&MenuAction::OpenSubmenu, ctx);
            menu.handle_action(&MenuAction::Select(SelectAction::Next), ctx);

            let submenu = menu.menu.selected_submenu().unwrap();
            assert_eq!(submenu.selected_index(), Some(1));
            assert_eq!(
                submenu.selected_item().unwrap().fields().unwrap().label(),
                "child two"
            );

            menu.handle_action(&MenuAction::Select(SelectAction::Previous), ctx);

            let submenu = menu.menu.selected_submenu().unwrap();
            assert_eq!(submenu.selected_index(), Some(0));
            assert_eq!(
                submenu.selected_item().unwrap().fields().unwrap().label(),
                "child one"
            );
        });
    })
}

#[test]
fn test_enter_uses_the_active_submenu_selection() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        let mut selected_action = None;
        menu.update(&mut app, |menu, ctx| {
            menu.set_selected_by_index(0, ctx);
            menu.handle_action(&MenuAction::OpenSubmenu, ctx);
            menu.handle_action(&MenuAction::Select(SelectAction::Next), ctx);

            selected_action = menu
                .menu
                .selected_action_for_enter(menu.submenu_position_namespace, ctx);
        });

        assert_eq!(selected_action, Some(TestAction::ChildTwo));
    })
}

#[test]
fn test_enter_action_can_rebuild_the_same_menu_after_current_update() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, parent) = app.add_window(WindowStyle::NotStealFocus, MenuRebuildingParent::new);
        let menu = parent.read(&app, |parent, _| parent.menu.clone());

        menu.update(&mut app, |menu, ctx| {
            menu.set_selected_by_index(0, ctx);
            menu.handle_action(&MenuAction::Enter, ctx);
        });

        parent.read(&app, |parent, _| {
            assert!(parent.handled);
            assert!(parent.closed);
            assert!(parent.action_observed_closed);
        });
        menu.read(&app, |menu, _| {
            assert_eq!(menu.items_len(), 1);
            assert_eq!(menu.items()[0].fields().unwrap().label(), "rebuilt");
        });
    })
}

#[test]
fn test_pointer_activation_uses_close_before_deferred_action() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, parent) = app.add_window(WindowStyle::NotStealFocus, MenuRebuildingParent::new);
        let menu = parent.read(&app, |parent, _| parent.menu.clone());

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::Activate {
                    depth: 0,
                    selection: SelectAction::Index { row: 0, item: 0 },
                },
                ctx,
            );
        });

        parent.read(&app, |parent, _| {
            assert!(parent.handled);
            assert!(parent.closed);
            assert!(parent.action_observed_closed);
        });
        menu.read(&app, |menu, _| {
            assert_eq!(menu.items()[0].fields().unwrap().label(), "rebuilt");
        });
    })
}

#[test]
fn test_nested_submenu_pointer_and_enter_resolve_the_same_action() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        for activation in [
            MenuAction::Enter,
            MenuAction::Activate {
                depth: 1,
                selection: SelectAction::Index { row: 0, item: 0 },
            },
        ] {
            let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
                let mut menu = Menu::<TestAction>::new().without_item_action_dispatch();
                menu.set_items(test_submenu_items(), ctx);
                menu
            });

            menu.update(&mut app, |menu, ctx| {
                menu.set_selected_by_index(0, ctx);
                menu.handle_action(&MenuAction::OpenSubmenu, ctx);
                menu.handle_action(&activation, ctx);
                assert_eq!(
                    menu.menu
                        .selected_action_for_enter(menu.submenu_position_namespace, ctx,),
                    Some(TestAction::ChildOne)
                );
            });
        }
    })
}

#[test]
fn test_activation_without_action_is_inert() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(vec![MenuItemFields::new("no action").into_item()], ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::Activate {
                    depth: 0,
                    selection: SelectAction::Index { row: 0, item: 0 },
                },
                ctx,
            );
            assert_eq!(menu.selected_index(), Some(0));
        });
    })
}

#[test]
fn test_right_is_a_noop_for_leaf_items() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.set_selected_by_index(1, ctx);
            menu.handle_action(&MenuAction::OpenSubmenu, ctx);

            assert_eq!(menu.selected_index(), Some(1));
            assert!(menu.menu.selected_submenu().is_none());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "root"
            );
        });
    })
}

#[test]
fn test_stale_submenu_parent_unhover_does_not_clear_new_hover_selection() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(two_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 1, item: 0 }),
                ctx,
            );
            menu.handle_action(&MenuAction::UnhoverSubmenuParent(0, 0), ctx);

            assert_eq!(menu.selected_index(), Some(1));
            assert_eq!(
                menu.selected_item()
                    .unwrap()
                    .submenu_fields()
                    .unwrap()
                    .label(),
                "second submenu"
            );
        });
    })
}

#[test]
fn test_submenu_parent_unhover_keeps_submenu_open() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            menu.handle_action(&MenuAction::UnhoverSubmenuParent(0, 0), ctx);

            assert_eq!(menu.selected_index(), Some(0));
            assert!(menu.menu.selected_submenu().is_some());
        });
    })
}

#[test]
fn test_leaf_hover_clears_previously_selected_submenu_parent() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            assert!(menu.menu.selected_submenu().is_some());

            menu.handle_action(
                &MenuAction::HoverSubmenuLeafNode {
                    depth: 0,
                    row_index: 1,
                    position: Default::default(),
                    select: true,
                },
                ctx,
            );

            assert_eq!(menu.selected_index(), Some(1));
            assert!(menu.menu.selected_submenu().is_none());
            assert_eq!(
                menu.selected_item().unwrap().fields().unwrap().label(),
                "root"
            );
        });
    })
}

#[test]
fn test_leaf_mouse_in_tracking_does_not_clear_open_submenu() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            menu.handle_action(
                &MenuAction::HoverSubmenuLeafNode {
                    depth: 0,
                    row_index: 1,
                    position: Default::default(),
                    select: false,
                },
                ctx,
            );

            assert_eq!(menu.selected_index(), Some(0));
            assert!(menu.menu.selected_submenu().is_some());
        });
    })
}

fn two_submenu_context_menu_items() -> Vec<MenuItem<TestAction>> {
    vec![
        MenuItem::Submenu {
            fields: MenuItemFields::new_submenu("upload"),
            menu: SubMenu::new(vec![MenuItemFields::new("upload file")
                .with_on_select_action(TestAction::ChildOne)
                .into_item()]),
        },
        MenuItem::Submenu {
            fields: MenuItemFields::new_submenu("other"),
            menu: SubMenu::new(vec![
                MenuItemFields::new("rename")
                    .with_on_select_action(TestAction::ChildOne)
                    .into_item(),
                MenuItemFields::new("copy name")
                    .with_on_select_action(TestAction::ChildTwo)
                    .into_item(),
            ]),
        },
    ]
}

#[test]
fn test_switching_submenu_parent_clears_nested_selection() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(two_submenu_context_menu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 1, item: 0 }),
                ctx,
            );
            menu.handle_action(
                &MenuAction::HoverSubmenuLeafNode {
                    depth: 1,
                    row_index: 1,
                    position: Default::default(),
                    select: true,
                },
                ctx,
            );
            assert_eq!(
                menu.menu
                    .selected_submenu()
                    .and_then(|submenu| submenu.selected_index()),
                Some(1)
            );

            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 1, item: 0 }),
                ctx,
            );

            assert_eq!(
                menu.menu
                    .selected_submenu()
                    .and_then(|submenu| submenu.selected_index()),
                None
            );
        });
    })
}

#[test]
fn test_nested_submenu_leaf_hover_is_handled_at_child_depth() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| Appearance::mock());

        let (_, menu) = app.add_window(WindowStyle::NotStealFocus, |ctx| {
            let mut menu = Menu::<TestAction>::new();
            menu.set_items(test_submenu_items(), ctx);
            menu
        });

        menu.update(&mut app, |menu, ctx| {
            menu.handle_action(
                &MenuAction::HoverSubmenuWithChildren(0, SelectAction::Index { row: 0, item: 0 }),
                ctx,
            );
            assert!(menu.menu.selected_submenu().is_some());

            menu.handle_action(
                &MenuAction::HoverSubmenuLeafNode {
                    depth: 1,
                    row_index: 0,
                    position: Default::default(),
                    select: true,
                },
                ctx,
            );

            assert_eq!(menu.selected_index(), Some(0));
            assert_eq!(
                menu.menu
                    .selected_submenu()
                    .and_then(|submenu| submenu.selected_index()),
                Some(0)
            );
            let child_label = menu
                .menu
                .selected_submenu()
                .and_then(|submenu| submenu.selected_item())
                .and_then(|item| item.fields().map(|fields| fields.label().to_string()));
            assert_eq!(child_label.as_deref(), Some("child one"));
        });
    })
}

#[test]
fn test_submenu_position_ids_are_scoped_per_menu_instance() {
    let first_menu = Menu::<()>::new();
    let second_menu = Menu::<()>::new();

    assert_ne!(
        first_menu.submenu_save_position_id_for_tests(0, 0),
        second_menu.submenu_save_position_id_for_tests(0, 0)
    );
}

#[test]
fn test_submenu_position_ids_are_scoped_per_row() {
    let menu = Menu::<()>::new();

    assert_ne!(
        menu.submenu_save_position_id_for_tests(0, 0),
        menu.submenu_save_position_id_for_tests(0, 1)
    );
}

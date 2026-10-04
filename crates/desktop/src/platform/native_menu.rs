//! One GPUI action schema, rendered by muda on both native desktop platforms.
use gpui_kit::{Action, App, OwnedMenu, OwnedMenuItem};
use muda::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::collections::HashMap;

pub(crate) struct NativeMenu {
    pub(crate) menu: Menu,
    actions: HashMap<MenuId, Box<dyn Action>>,
}

impl NativeMenu {
    pub(crate) fn new(cx: &App) -> anyhow::Result<Self> {
        let mut native = Self {
            menu: Menu::new(),
            actions: HashMap::new(),
        };
        for menu in super::application_menus() {
            let submenu = native.submenu(menu, cx)?;
            native.menu.append(&submenu)?;
        }
        Ok(native)
    }

    fn submenu(&mut self, menu: OwnedMenu, cx: &App) -> anyhow::Result<Submenu> {
        let submenu = Submenu::new(menu.name, !menu.disabled);
        for item in menu.items {
            match item {
                OwnedMenuItem::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                OwnedMenuItem::Submenu(menu) => submenu.append(&self.submenu(menu, cx)?)?,
                OwnedMenuItem::SystemMenu(_) => anyhow::bail!("Unsupported system menu"),
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => {
                    // Stable action IDs survive menu refreshes. Tray items have
                    // their own namespace but share muda's single event handler.
                    let id = MenuId::new(format!("listenbox.app.{}", action.name()));
                    let accelerator = accelerator(action.as_ref(), cx);
                    if checked {
                        let item =
                            muda::CheckMenuItem::with_id(id.clone(), name, !disabled, true, None);
                        item.set_key_accelerator(accelerator)?;
                        submenu.append(&item)?;
                    } else {
                        let item = MenuItem::with_id(id.clone(), name, !disabled, None);
                        item.set_key_accelerator(accelerator)?;
                        submenu.append(&item)?;
                    }
                    if !disabled && !menu.disabled {
                        self.actions.insert(id, action);
                    }
                }
            }
        }
        Ok(submenu)
    }

    pub(crate) fn action(&self, id: &MenuId) -> Option<Box<dyn Action>> {
        self.actions.get(id).map(|action| action.boxed_clone())
    }
}

fn accelerator(action: &dyn Action, cx: &App) -> Option<muda::accelerator::KeyAccelerator> {
    use muda::accelerator::{Key, KeyAccelerator, Modifiers};
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap
        .bindings_for_action(action)
        .find(|binding| binding.predicate().is_none() && binding.keystrokes().len() == 1)?;
    let keystroke = binding.keystrokes()[0].inner();
    // The app's menu shortcuts use character keys. GPUI still handles its
    // contextual/chord bindings, including the guarded macOS quit shortcut.
    if keystroke.key.chars().count() != 1 || keystroke.modifiers.function {
        return None;
    }
    let mut modifiers = Modifiers::empty();
    for (pressed, flag) in [
        (keystroke.modifiers.control, Modifiers::CONTROL),
        (keystroke.modifiers.alt, Modifiers::ALT),
        (keystroke.modifiers.shift, Modifiers::SHIFT),
        (keystroke.modifiers.platform, Modifiers::META),
    ] {
        if pressed {
            modifiers |= flag;
        }
    }
    Some(KeyAccelerator::new(
        modifiers,
        Key::Character(keystroke.key.clone()),
    ))
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) const OPEN_WINDOW: &str = "listenbox.tray.open";
#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(super) const QUIT: &str = "listenbox.tray.quit";

#[cfg(any(target_os = "macos", target_os = "windows", test))]
mod installed {
    use super::*;
    use gpui_kit::{Global, Window};
    use std::{cell::RefCell, rc::Rc};

    struct ApplicationMenu {
        current: Rc<RefCell<Option<NativeMenu>>>,
        #[cfg(target_os = "windows")]
        hwnd: isize,
    }
    impl Global for ApplicationMenu {}

    pub(crate) fn install(
        window: &Window,
        cx: &mut App,
    ) -> tokio::sync::mpsc::UnboundedSender<MenuId> {
        let current = Rc::new(RefCell::new(None));
        cx.set_global(ApplicationMenu {
            current: current.clone(),
            #[cfg(target_os = "windows")]
            hwnd: super::super::windows::hwnd(window),
        });
        let _ = window;
        // muda permits exactly one event handler for app and tray menus.
        // Queue native callbacks so dispatch never re-enters a GPUI App borrow.
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let events = sender.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            let _ = events.send(event.id);
        }));
        cx.spawn(async move |cx| {
            while let Some(id) = receiver.recv().await {
                cx.update(|cx| {
                    if id == OPEN_WINDOW {
                        super::super::show_window(cx);
                    } else if id == QUIT {
                        cx.dispatch_action(&super::super::Quit);
                    } else {
                        let action = current.borrow().as_ref().and_then(|menu| menu.action(&id));
                        if let Some(action) = action {
                            cx.dispatch_action(action.as_ref());
                        }
                    }
                });
            }
        })
        .detach();
        refresh(cx);
        sender
    }

    pub(crate) fn refresh(cx: &mut App) {
        let Some(installed) = cx.try_global::<ApplicationMenu>() else {
            return;
        };
        let current = installed.current.clone();
        #[cfg(target_os = "windows")]
        let hwnd = installed.hwnd;
        let menu = NativeMenu::new(cx).expect("Listenbox native menus");
        // Attaching or dropping a Win32 menu can synchronously send WM_SIZE.
        // Keep every native window mutation outside GPUI's current App borrow.
        cx.foreground_executor()
            .spawn(async move {
                drop(current.borrow_mut().take());
                #[cfg(target_os = "macos")]
                menu.menu.init_for_nsapp();
                #[cfg(target_os = "windows")]
                unsafe {
                    // SAFETY: the HWND belongs to GPUI's retained workspace window.
                    menu.menu
                        .init_for_hwnd(hwnd)
                        .expect("Listenbox Windows menu bar");
                }
                current.replace(Some(menu));
            })
            .detach();
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
pub(crate) use installed::{install, refresh};

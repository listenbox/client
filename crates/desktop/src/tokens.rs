//! Native projection of Listenbox's DESIGN.md palette and typography.
//! New screens consume these roles rather than introducing local colors or scales.
use gpui_kit::component::{ActiveTheme, Theme, ThemeMode};
use gpui_kit::{App, Hsla, Window, px, rgb};

pub const SIDEBAR: f32 = 240.;
pub const SIDEBAR_INSET: f32 = 16.;
pub const NAV_ROW_INSET: f32 = 8.;
pub const CONTROL_HEIGHT: f32 = 36.;
pub const PAGE_TITLE: f32 = 30.;
pub const TITLE: f32 = 18.;
pub const BODY: f32 = 14.;
pub const SPACE: f32 = 24.;
pub const GAP: f32 = 12.;
pub const RADIUS: f32 = 10.;
pub const QUIT_HUD_WIDTH: f32 = 280.;
pub const QUIT_HUD_RADIUS: f32 = 20.;
pub const QUIT_SHORTCUT: f32 = 56.;

#[derive(Clone, Copy)]
pub struct Tokens {
    pub background: Hsla,
    pub sheet: Hsla,
    pub rail: Hsla,
    pub ink: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub divider: Hsla,
    pub selected: Hsla,
    pub plan: Hsla,
    pub action: Hsla,
    pub action_ink: Hsla,
    pub danger: Hsla,
}

fn neutral(lightness: f32) -> Hsla {
    let linear = lightness.powi(3);
    let srgb = if linear <= 0.0031308 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    };
    gpui_kit::hsla(0., 0., srgb, 1.)
}

impl Tokens {
    pub fn current(cx: &App) -> Self {
        if cx.theme().mode == ThemeMode::Dark {
            Self {
                background: neutral(0.2134),
                sheet: neutral(0.252),
                rail: neutral(0.2478),
                ink: neutral(0.9702),
                muted: neutral(0.8015),
                border: neutral(0.4349),
                divider: neutral(0.3446),
                selected: neutral(0.3368),
                plan: neutral(0.4532),
                action: neutral(0.54),
                action_ink: neutral(1.),
                danger: rgb(0xef9a8d).into(),
            }
        } else {
            Self {
                background: neutral(0.9851),
                sheet: neutral(1.),
                rail: neutral(0.9612),
                ink: neutral(0.2435),
                muted: neutral(0.4532),
                border: neutral(0.8761),
                divider: neutral(0.9219),
                selected: neutral(0.9219),
                plan: neutral(0.3092),
                action: neutral(0.3092),
                action_ink: neutral(1.),
                danger: rgb(0xb44d42).into(),
            }
        }
    }
}

pub fn apply(window: &mut Window, cx: &mut App) {
    Theme::sync_system_appearance(Some(window), cx);
    project(cx);
}

pub fn project(cx: &mut App) {
    let t = Tokens::current(cx);
    let theme = Theme::global_mut(cx);
    theme.font_size = px(BODY);
    theme.radius = px(RADIUS);
    theme.colors.background = t.background;
    theme.colors.foreground = t.ink;
    theme.colors.muted = t.rail;
    theme.colors.muted_foreground = t.muted;
    theme.colors.border = t.border;
    theme.colors.input = if theme.is_dark() {
        neutral(0.62)
    } else {
        neutral(0.6167)
    };
    theme.colors.caret = t.ink;
    theme.colors.ring = t.action;
    theme.colors.selection = t.selected;
    theme.colors.primary = t.action;
    theme.colors.primary_foreground = t.action_ink;
    theme.colors.button_primary = t.action;
    theme.colors.button_primary_hover = if theme.is_dark() {
        neutral(0.56)
    } else {
        t.ink
    };
    theme.colors.button_primary_active = t.action;
    theme.colors.button_primary_foreground = t.action_ink;
    theme.colors.button = t.sheet;
    theme.colors.button_hover = t.selected;
    theme.colors.button_foreground = t.ink;
    theme.colors.accent = t.selected;
    theme.colors.accent_foreground = t.ink;
    theme.colors.sidebar = t.rail;
    theme.colors.sidebar_foreground = t.ink;
    // GPUI Kit paints component backgrounds through its richer token layer;
    // foregrounds and Base scrollbars must receive the same projection.
    theme.tokens = (&theme.colors).into();
    Theme::sync_base(cx);
}

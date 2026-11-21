//! The look of the main window, shared with Vifte: colour tokens for light
//! and dark, static "glass" (a dark or light background with a faint blue
//! glow and half-transparent cards), and Vifte's orange as the accent.
//! Dialogs stay outside the glass and keep the plain, solid look.
//!
//! Only `@define-color`, `alpha()` and `mix()`: `var()` and `color-mix()` need
//! a newer GTK than 4.14.

use std::cell::RefCell;

use adw::prelude::*;
use gtk::gdk;

const ACCENT_FG: &str = "#1D1D1F";

/// (name, dark, light).
const PALETTE: &[(&str, &str, &str)] = &[
    ("stress_cpu", "#7AA2D2", "#2F5A8A"),
    ("stress_cpu_surface", "#34506E", "#34506E"),
    ("stress_gpu", "#D29A6C", "#9A5B26"),
    ("stress_gpu_surface", "#9A6236", "#9A6236"),
    ("stress_neutral", "#DCDEE3", "#2E3138"),
    ("stress_neutral_surface", "#4A505C", "#5B616C"),
    ("stress_accent", "#F0A400", "#C47F00"),
    ("stress_accent_text", "#F5B83D", "#8F5D00"),
    ("stress_text", "#F5F5F7", "#1D1D1F"),
    ("stress_sub", "#B4B8C2", "#55585E"),
    ("stress_sidebar", "alpha(white, 0.075)", "alpha(white, 0.62)"),
    ("stress_card", "alpha(white, 0.055)", "alpha(white, 0.66)"),
    ("stress_fill", "alpha(white, 0.08)", "alpha(black, 0.055)"),
    ("stress_track", "alpha(white, 0.10)", "alpha(black, 0.08)"),
    ("stress_line", "alpha(white, 0.09)", "alpha(black, 0.08)"),
    ("stress_selected", "alpha(#F0A400, 0.20)", "alpha(#C47F00, 0.16)"),
    ("stress_band", "alpha(#F0A400, 0.07)", "alpha(#C47F00, 0.08)"),
    ("stress_stop", "alpha(#E01B24, 0.22)", "alpha(#E01B24, 0.12)"),
    ("stress_stop_text", "#FF8A80", "#B3141B"),
    ("stress_calm", "#2EC27E", "#2EC27E"),
    ("stress_warm", "#E5A50A", "#E5A50A"),
    ("stress_hot", "#E66100", "#E66100"),
    ("stress_emergency", "#E01B24", "#E01B24"),
];

const CSS: &str = r#"
.reading { font-feature-settings: "tnum"; }
.stress-big { font-size: 30px; font-weight: 700; letter-spacing: -0.8px; font-feature-settings: "tnum"; }
.stress-huge { font-size: 46px; font-weight: 700; letter-spacing: -1.4px; font-feature-settings: "tnum"; }
.stress-unit { font-size: 16px; font-weight: 600; }
.stress-value { font-size: 15px; font-weight: 700; font-feature-settings: "tnum"; }
.stress-sub { font-feature-settings: "tnum"; }

/* Glass: only in the main window, and not in high contrast (then the class stress-glass is missing). */
.stress-root.stress-glass {
  color: @stress_text;
  background-color: #0D1016;
  background-image: radial-gradient(75% 65% at 88% -12%, alpha(#6080BE, 0.20), transparent 70%),
                    radial-gradient(55% 45% at 0% 100%, alpha(#465A82, 0.12), transparent 70%),
                    linear-gradient(165deg, #141821, #0D1016 55%, #090B10);
}
.stress-root.stress-glass.stress-light {
  background-color: #E8EBF0;
  background-image: radial-gradient(75% 65% at 88% -12%, alpha(#7896CD, 0.22), transparent 70%),
                    linear-gradient(165deg, #F2F4F8, #E8EBF0);
}
.stress-glass headerbar { background: none; box-shadow: none; }
.stress-glass .dim-label { opacity: 1; color: @stress_sub; }
.stress-glass .stress-unit { color: @stress_sub; }
.stress-glass separator { background-color: @stress_line; }

/* The tint after the temperature (off by default): a layer over the static background. */
.stress-glass .stress-main { transition: background-image 1200ms ease; }
.stress-glass .stress-main.stress-tint-cool { background-image: radial-gradient(farthest-side at 10% 0, alpha(#4a8fd0, 0.30), transparent); }
.stress-glass .stress-main.stress-tint-mild { background-image: radial-gradient(farthest-side at 10% 0, alpha(#48a89a, 0.28), transparent); }
.stress-glass .stress-main.stress-tint-warm { background-image: radial-gradient(farthest-side at 10% 0, alpha(#d9a03a, 0.30), transparent); }
.stress-glass .stress-main.stress-tint-hot { background-image: radial-gradient(farthest-side at 10% 0, alpha(#de6a3a, 0.34), transparent); }

/* Page header and sidebar */
.stress-page-header { min-height: 72px; }
.stress-page-title { font-size: 26px; font-weight: 800; letter-spacing: -0.6px; }
.stress-page-subtitle { font-size: 13px; }
.stress-glass .stress-main .sidebar-pane, .stress-glass .stress-main .content-pane,
.stress-glass .stress-main navigation-view > * { background: none; }
.stress-glass .stress-sidebar { background-color: @stress_sidebar; box-shadow: inset -1px 0 @stress_line; }
.stress-glass .stress-main navigation-view .stress-sidebar { box-shadow: none; }
.stress-nav { background: none; }
.stress-nav > row { min-height: 36px; padding: 0 10px; border-radius: 9px; }
.stress-glass .stress-nav image { color: @stress_sub; }
.stress-glass .stress-nav > row:selected { background-color: @stress_selected; }
.stress-glass .stress-nav > row:selected image { color: @stress_accent_text; }
.stress-nav > row:selected label { font-weight: 600; }
.stress-nav-dot { min-width: 7px; min-height: 7px; border-radius: 99px; background-color: @stress_warm; }

.stress-status { padding: 10px 12px; border-radius: 12px; }
.stress-glass .stress-status { background-color: @stress_fill; }
.stress-status-title { font-size: 12.5px; font-weight: 700; }
.stress-status-icon { min-width: 26px; min-height: 26px; border-radius: 99px; color: white; background-color: @stress_calm; }
.stress-status-icon.warm, .stress-status-icon.running { background-color: @stress_warm; }
.stress-status-icon.claude { background-color: #d97757; }
.stress-status-icon.idle { background-color: @stress_neutral_surface; }
.stress-status-icon.emergency, .stress-status-icon.stopped { background-color: @stress_emergency; }

/* Cards */
.stress-icon-surface { min-width: 24px; min-height: 24px; border-radius: 7px; color: white; background-color: #4A505C; }
.stress-icon-surface.small { min-width: 22px; min-height: 22px; }
.stress-icon-surface.cpu { background-color: #34506E; }
.stress-icon-surface.gpu { background-color: #9A6236; }
.stress-icon-surface.accent { background-color: @accent_bg_color; color: @accent_fg_color; }
.stress-glass .stress-icon-surface.neutral { background-color: @stress_neutral_surface; }
.stress-card { padding: 14px 16px; }
.stress-tile { padding: 14px 16px; min-width: 0; }
.stress-bottom-card { padding: 14px 18px 8px 18px; }
.stress-glass .stress-card, .stress-glass .stress-list {
  background-color: @stress_card; border-radius: 16px;
  box-shadow: inset 0 1px alpha(white, 0.09), inset 0 0 0 1px alpha(white, 0.06), 0 12px 32px alpha(black, 0.35);
}
.stress-glass.stress-light .stress-card, .stress-glass.stress-light .stress-list {
  box-shadow: inset 0 1px alpha(white, 0.9), inset 0 0 0 1px alpha(white, 0.55), 0 10px 28px alpha(#1E283C, 0.10);
}
.stress-glass .stress-list { border-radius: 14px; }
.stress-link { padding: 2px 6px; font-weight: 600; }
.stress-glass .stress-link { color: @stress_accent_text; }
.stress-legend { min-width: 14px; min-height: 3px; border-radius: 99px; background-color: @stress_cpu; }
.stress-legend.gpu { background-color: @stress_gpu; }
.stress-legend.band { min-height: 10px; border-radius: 3px; background-color: @stress_selected; }
.stress-legend.limit { background-color: @stress_emergency; }
.stress-summary { min-height: 54px; margin-top: 4px; border-bottom: 1px solid alpha(currentColor, 0.1); }
.stress-glass .stress-summary { border-bottom-color: @stress_line; }
.stress-summary-value { font-size: 26px; }
.stress-summary .stress-status-icon { min-width: 24px; min-height: 24px; border-radius: 7px; }
.stress-overview-row { min-height: 48px; }
.stress-overview-rows > :not(:first-child) { border-top: 1px solid alpha(currentColor, 0.1); }
.stress-glass .stress-overview-rows > :not(:first-child) { border-top-color: @stress_line; }
.stress-power-row { min-height: 44px; }
.stress-power-row .stress-icon-surface { background-color: alpha(currentColor, 0.1); color: inherit; }
.stress-glass .stress-power-row .stress-icon-surface { background-color: @stress_fill; color: @stress_neutral; }

/* Lists */
.stress-group-label { font-size: 14px; font-weight: 700; }
.stress-list > row.stress-row { min-height: 56px; padding: 0 14px; }
.stress-list > row.stress-detail-row { min-height: 44px; padding: 0 14px; }
.stress-row-title { font-weight: 600; }
.stress-list .stress-icon-surface.row { min-width: 30px; min-height: 30px; border-radius: 9px; background-color: alpha(currentColor, 0.1); color: inherit; }
.stress-glass .stress-list .stress-icon-surface.row { background-color: @stress_fill; color: @stress_neutral; }
.stress-glass .stress-list > row { background: none; border-color: @stress_line; }
.stress-glass .stress-list > row:hover { background-color: alpha(currentColor, 0.03); }
.stress-glass .stress-list > row.activatable:hover { background-color: alpha(currentColor, 0.05); }

/* Status words */
.stress-chip { font-size: 11px; font-weight: 700; padding: 1px 8px; border-radius: 99px; background-color: alpha(@window_fg_color, 0.1); }
.stress-glass .stress-chip { background-color: @stress_fill; }
.stress-dot { min-width: 7px; min-height: 7px; border-radius: 99px; background-color: @stress_calm; }
.stress-dot.warm, .stress-dot.running { background-color: @stress_warm; }
.stress-dot.hot { background-color: @stress_hot; }
.stress-dot.stopped { background-color: @stress_emergency; }
.stress-dot.idle { background-color: @stress_neutral_surface; }
.stress-dot.claude { background-color: #d97757; }

/* Bars: 6 px, fully round */
.stress-card levelbar trough, .stress-card levelbar block { min-height: 6px; border-radius: 99px; }
.stress-glass .stress-card levelbar block.empty { background-color: @stress_track; }
.stress-glass .stress-card levelbar.cpu block.filled { background-color: @stress_cpu; }
.stress-glass .stress-card levelbar.gpu block.filled { background-color: @stress_gpu; }
.stress-glass .stress-card levelbar.neutral block.filled { background-color: @stress_neutral; }
.stress-progress trough { min-height: 8px; border-radius: 99px; }
.stress-progress progress { min-height: 8px; border-radius: 99px; background-color: @accent_bg_color; }
.stress-progress.thin trough, .stress-progress.thin progress { min-height: 4px; }
.stress-glass .stress-progress trough { background-color: @stress_track; }

/* Load test */
.stress-go { padding: 10px; border-radius: 12px; font-size: 15px; font-weight: 700; }
.stress-glass .stress-stop { background-color: @stress_stop; color: @stress_stop_text; }
.stress-glass .stress-stop:hover { background-color: mix(@stress_stop, @stress_stop_text, 0.12); }
.stress-profile { padding: 9px 12px; border-radius: 12px; background-color: alpha(currentColor, 0.06); }
.stress-glass .stress-profile { background-color: @stress_fill; }
.stress-profile:hover { background-color: alpha(currentColor, 0.11); }
.stress-profile:checked { background-color: @stress_selected; color: inherit; box-shadow: inset 0 0 0 1.5px @accent_bg_color; }
.stress-profile-name { font-size: 13px; font-weight: 700; }
.stress-profile-load { font-size: 11.5px; font-feature-settings: "tnum"; }
.stress-glass .stress-profile-load { color: @stress_sub; }
.stress-glass .stress-profile:checked .stress-profile-load { color: @stress_accent_text; }
.stress-foot { font-size: 12px; }

/* Status cards in dialogs (preferences) */
.status-hero {
  padding: 12px 14px;
  border-radius: 14px;
  box-shadow: inset 0 0 0 1px alpha(currentColor, 0.08);
  background-image: linear-gradient(115deg, alpha(@stress_calm, 0.18), alpha(currentColor, 0.04));
}
.status-hero.idle { background-image: linear-gradient(115deg, alpha(currentColor, 0.1), alpha(currentColor, 0.04)); }
.status-hero.claude { background-image: linear-gradient(115deg, alpha(#d97757, 0.24), alpha(currentColor, 0.04)); }
.status-hero-dot { min-width: 10px; min-height: 10px; border-radius: 99px; background-color: @stress_calm; }
.status-hero.idle .status-hero-dot { background-color: alpha(currentColor, 0.5); }
.status-hero.claude .status-hero-dot { background-color: #d97757; }
.status-hero-title { font-size: 15px; font-weight: 700; }
.status-hero-text { font-size: 13px; opacity: 0.75; font-feature-settings: "tnum"; }
"#;

thread_local! {
    static PALETTE_PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

/// The colour tokens and the accent for light or dark.
fn palette_css(dark: bool) -> String {
    let mut css: String = PALETTE
        .iter()
        .map(|(name, d, l)| format!("@define-color {name} {};\n", if dark { d } else { l }))
        .collect();
    let (accent, text) = if dark { ("#F0A400", "#F5B83D") } else { ("#C47F00", "#8F5D00") };
    css += &format!("@define-color accent_bg_color {accent};\n@define-color accent_color {text};\n@define-color accent_fg_color {ACCENT_FG};\n");
    // From GTK 4.16, libadwaita 1.6 reads the accent from CSS variables.
    if gtk::check_version(4, 16, 0).is_none() {
        css += &format!(":root {{ --accent-bg-color: {accent}; --accent-color: {text}; --accent-fg-color: {ACCENT_FG}; }}\n");
    }
    css
}

pub fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let palette = gtk::CssProvider::new();
    let style = adw::StyleManager::default();
    palette.load_from_string(&palette_css(style.is_dark()));
    gtk::style_context_add_provider_for_display(&display, &palette, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    style.connect_dark_notify(|style| {
        PALETTE_PROVIDER.with(|p| {
            if let Some(provider) = p.borrow().as_ref() {
                provider.load_from_string(&palette_css(style.is_dark()));
            }
        });
    });
    PALETTE_PROVIDER.with(|p| *p.borrow_mut() = Some(palette));
}

/// Light or dark picks the variant of the glass; in high contrast the glass
/// is off (plain Adwaita cards).
pub fn follow_style(root: &impl IsA<gtk::Widget>) {
    let weak = root.clone().upcast::<gtk::Widget>().downgrade();
    let apply = move |style: &adw::StyleManager| {
        let Some(root) = weak.upgrade() else { return };
        for (name, on) in [("stress-light", !style.is_dark()), ("stress-glass", !style.is_high_contrast())] {
            if on {
                root.add_css_class(name);
            } else {
                root.remove_css_class(name);
            }
        }
    };
    let style = adw::StyleManager::default();
    apply(&style);
    let a = apply.clone();
    style.connect_dark_notify(move |s| a(s));
    style.connect_high_contrast_notify(move |s| apply(s));
}

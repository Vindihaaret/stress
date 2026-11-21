//! Building blocks shared by the pages, as in Vifte: status chips, icon
//! surfaces, large numbers with their unit, small sparklines for the last two
//! minutes and the large temperature graph with load tests and the limit.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, gio};

use crate::i18n::tr;

/// Readings as (seconds ago, value), oldest first.
pub type Points = Vec<(f64, Option<f64>)>;

/// Seconds a sparkline spans.
pub const SPARK_SPAN: f64 = 120.0;

/// Icons with fallbacks: the first one the theme has (Adwaita has no
/// processor icon, for one). Same choices as Vifte.
pub const ICON_CPU: &[&str] = &["cpu-symbolic", "computer-chip-symbolic", "computer-symbolic"];
pub const ICON_GPU: &[&str] = &["graphics-card-symbolic", "video-display-symbolic"];
pub const ICON_PSU: &[&str] = &["ac-adapter-symbolic", "battery-ac-adapter-symbolic", "battery-full-charged-symbolic"];
pub const ICON_TEST: &[&str] = &["speedometer-symbolic", "power-profile-performance-symbolic"];
pub const ICON_HOME: &[&str] = &["go-home-symbolic"];
pub const ICON_TEMPERATURE: &[&str] = &["temperature-symbolic"];
pub const ICON_LOAD: &[&str] = &["speedometer-symbolic", "power-profile-performance-symbolic"];
pub const ICON_POWER: &[&str] = &["thunderbolt-symbolic", "battery-level-100-charged-symbolic"];
pub const ICON_FAN: &[&str] = &["fan-symbolic", "weather-windy-symbolic"];
pub const ICON_REST: &[&str] = &["computer-symbolic"];
pub const ICON_TIME: &[&str] = &["alarm-symbolic", "preferences-system-time-symbolic"];
pub const ICON_INFO: &[&str] = &["dialog-information-symbolic"];

/// An identity colour: processor, graphics card, or neither.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Cpu,
    Gpu,
    Neutral,
}

impl Tone {
    pub fn class(self) -> &'static str {
        match self {
            Tone::Cpu => "cpu",
            Tone::Gpu => "gpu",
            Tone::Neutral => "neutral",
        }
    }

    fn color_name(self) -> &'static str {
        match self {
            Tone::Cpu => "stress_cpu",
            Tone::Gpu => "stress_gpu",
            Tone::Neutral => "stress_neutral",
        }
    }
}

pub fn image(names: &[&str], size: i32) -> gtk::Image {
    gtk::Image::builder().gicon(&gio::ThemedIcon::from_names(names)).pixel_size(size).build()
}

/// A small coloured surface with a white icon: processor blue, graphics card
/// copper, otherwise neutral grey.
pub fn icon_surface(names: &[&str], kind: &str, small: bool) -> gtk::Image {
    let icon = image(names, if small { 13 } else { 14 });
    icon.set_valign(gtk::Align::Center);
    icon.set_css_classes(&["stress-icon-surface", kind]);
    if small {
        icon.add_css_class("small");
    }
    icon
}

/// A small status word with a coloured dot.
pub struct Chip {
    pub widget: gtk::Box,
    dot: gtk::Box,
    label: gtk::Label,
}

impl Chip {
    pub fn new() -> Chip {
        let widget = gtk::Box::builder().spacing(5).valign(gtk::Align::Center).css_classes(["stress-chip"]).build();
        let dot = gtk::Box::builder().valign(gtk::Align::Center).css_classes(["stress-dot"]).build();
        let label = gtk::Label::new(None);
        widget.append(&dot);
        widget.append(&label);
        Chip { widget, dot, label }
    }

    /// `level` is calm, warm, hot, running, stopped or idle; the word
    /// carries the meaning, so colour is never the only sign.
    pub fn set(&self, level: &str, text: &str) {
        self.dot.set_css_classes(&["stress-dot", level]);
        self.label.set_label(text);
    }
}

/// A large number with a smaller unit after it.
pub struct BigValue {
    pub widget: gtk::Box,
    value: gtk::Label,
    unit: gtk::Label,
}

impl BigValue {
    pub fn new(class: &str) -> BigValue {
        let widget = gtk::Box::builder().spacing(4).valign(gtk::Align::End).build();
        let value = gtk::Label::builder().label("–").valign(gtk::Align::Baseline).css_classes([class]).build();
        let unit = gtk::Label::builder().valign(gtk::Align::Baseline).css_classes(["stress-unit"]).build();
        widget.append(&value);
        widget.append(&unit);
        BigValue { widget, value, unit }
    }

    pub fn set_parts(&self, value: &str, unit: &str) {
        self.value.set_label(value);
        self.unit.set_label(unit);
    }

    /// "41.8 °C" → large "41.8" and a smaller "°C".
    pub fn set(&self, text: &str) {
        match text.rsplit_once(' ') {
            Some((number, unit)) if !number.is_empty() => {
                self.value.set_label(number);
                self.unit.set_label(unit);
            }
            _ => {
                self.value.set_label(text);
                self.unit.set_label("");
            }
        }
    }
}

/// "Temperature · last 30 minutes" with a legend on the right.
pub fn graph_heading(title: &str, period: &str, legend: &[(&str, &str)]) -> gtk::Box {
    let head = gtk::Box::builder().spacing(6).build();
    head.append(&gtk::Label::builder().label(title).css_classes(["heading"]).build());
    head.append(&gtk::Label::builder().label(format!("· {period}")).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).build());
    for (class, name) in legend {
        let item = gtk::Box::builder().spacing(6).margin_start(10).build();
        item.append(&gtk::Box::builder().valign(gtk::Align::Center).css_classes(["stress-legend", class]).build());
        item.append(&gtk::Label::builder().label(*name).css_classes(["caption"]).build());
        head.append(&item);
    }
    head
}

/// A card: half-transparent in the glass, a normal Adwaita card otherwise.
pub fn card(spacing: i32, class: &str) -> gtk::Box {
    gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(spacing).hexpand(true).css_classes(["card", "stress-card", class]).build()
}

/// "Controls ›": the link at the top right of the overview's cards.
pub fn link_button(label: &str) -> gtk::Button {
    let button = gtk::Button::builder().valign(gtk::Align::Center).css_classes(["flat", "stress-link"]).build();
    let content = gtk::Box::builder().spacing(4).build();
    content.append(&gtk::Label::new(Some(label)));
    content.append(&gtk::Image::builder().icon_name("go-next-symbolic").pixel_size(12).build());
    button.set_child(Some(&content));
    button
}

#[allow(deprecated)] // lookup_color has no replacement that works with libadwaita 1.5.
fn named_color(widget: &impl IsA<gtk::Widget>, name: &str, fallback: gdk::RGBA) -> gdk::RGBA {
    widget.as_ref().style_context().lookup_color(name).unwrap_or(fallback)
}

const GREY: gdk::RGBA = gdk::RGBA::new(0.5, 0.5, 0.55, 1.0);

fn rgb(c: &gdk::RGBA) -> (f64, f64, f64) {
    (c.red() as f64, c.green() as f64, c.blue() as f64)
}

/// Splits points into runs at missing readings and at gaps (the computer
/// slept), each as screen coordinates.
fn runs(points: &[(f64, Option<f64>)], xy: impl Fn(f64, f64) -> (f64, f64)) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut last_ago = f64::INFINITY;
    for &(ago, value) in points {
        match value {
            Some(v) if last_ago - ago < 3.0 && !runs.is_empty() => runs.last_mut().unwrap().push(xy(ago, v)),
            Some(v) => runs.push(vec![xy(ago, v)]),
            None => {}
        }
        last_ago = if value.is_some() { ago } else { f64::INFINITY };
    }
    runs
}

/// A line with an area under it that fades out towards `bottom`.
fn draw_runs(cr: &cairo::Context, runs: &[Vec<(f64, f64)>], color: &gdk::RGBA, top: f64, bottom: f64, fill: f64) {
    let (r, g, b) = rgb(color);
    let fade = cairo::LinearGradient::new(0.0, top, 0.0, bottom);
    fade.add_color_stop_rgba(0.0, r, g, b, fill);
    fade.add_color_stop_rgba(1.0, r, g, b, 0.0);
    for run in runs {
        let (first, last) = (run[0], run[run.len() - 1]);
        cr.move_to(first.0, bottom);
        for &(x, y) in run {
            cr.line_to(x, y);
        }
        cr.line_to(last.0, bottom);
        cr.close_path();
        let _ = cr.set_source(&fade);
        let _ = cr.fill();
        cr.move_to(first.0, first.1);
        for &(x, y) in &run[1..] {
            cr.line_to(x, y);
        }
        if run.len() == 1 {
            cr.line_to(first.0 + 1.0, first.1);
        }
        cr.set_source_rgba(r, g, b, 1.0);
        cr.set_line_width(2.0);
        cr.set_line_join(cairo::LineJoin::Round);
        let _ = cr.stroke();
    }
}

fn end_dot(cr: &cairo::Context, (x, y): (f64, f64), color: &gdk::RGBA, radius: f64) {
    let (r, g, b) = rgb(color);
    cr.arc(x, y, radius, 0.0, std::f64::consts::TAU);
    cr.set_source_rgba(r, g, b, 1.0);
    let _ = cr.fill();
}

/// A small graph of the last two minutes.
pub struct Sparkline {
    pub area: gtk::DrawingArea,
    points: Rc<RefCell<Points>>,
}

impl Sparkline {
    /// `min_span` is the least height of the vertical axis, so small wobbles
    /// stay small.
    pub fn new(tone: Tone, width: i32, height: i32, dot: bool, min_span: f64) -> Sparkline {
        let area = gtk::DrawingArea::builder().content_width(width).content_height(height).valign(gtk::Align::Center).build();
        let points: Rc<RefCell<Points>> = Rc::default();
        let shared = points.clone();
        area.set_draw_func(move |area, cr, w, h| {
            let points = shared.borrow();
            let values: Vec<f64> = points.iter().filter_map(|(_, v)| *v).collect();
            if values.is_empty() {
                return;
            }
            let (w, h) = (w as f64, h as f64);
            let (mut lo, mut hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            if hi - lo < min_span {
                let mid = (hi + lo) / 2.0;
                (lo, hi) = (mid - min_span / 2.0, mid + min_span / 2.0);
            }
            let xy = |ago: f64, v: f64| (1.0 + (w - 5.0) * (1.0 - ago / SPARK_SPAN), 3.0 + (h - 6.0) * (1.0 - (v - lo) / (hi - lo)));
            let color = named_color(area, tone.color_name(), GREY);
            let runs = runs(&points, xy);
            draw_runs(cr, &runs, &color, 0.0, h, 0.35);
            if let (true, Some(&(ago, Some(v)))) = (dot, points.last()) {
                end_dot(cr, xy(ago, v), &color, 2.6);
            }
        });
        Sparkline { area, points }
    }

    /// Keeps the last two minutes of `points`.
    pub fn set(&self, points: &[(f64, Option<f64>)]) {
        let recent: Points = points.iter().filter(|(ago, _)| *ago <= SPARK_SPAN).copied().collect();
        *self.points.borrow_mut() = recent;
        self.area.queue_draw();
    }
}

#[derive(Default)]
struct GraphData {
    series: Vec<(Tone, Points)>,
    /// When a load test ran, as (started, ended) in seconds ago.
    tests: Vec<(f64, f64)>,
    limit: Option<f64>,
}

/// The large temperature graph: lines over the last minutes, load tests as a
/// band behind them and the limit as a dashed red line.
#[derive(Clone)]
pub struct HistoryGraph {
    pub area: gtk::DrawingArea,
    data: Rc<RefCell<GraphData>>,
}

const PAD_L: f64 = 46.0;
const PAD_R: f64 = 10.0;
const PAD_T: f64 = 8.0;
const PAD_B: f64 = 26.0;

impl HistoryGraph {
    /// `span` is the seconds across the width; `x_labels` are spread evenly
    /// from the oldest to now.
    pub fn new(height: i32, span: f64, x_labels: Vec<String>) -> HistoryGraph {
        let area = gtk::DrawingArea::builder().content_height(height).hexpand(true).build();
        let data: Rc<RefCell<GraphData>> = Rc::default();
        let shared = data.clone();
        area.set_draw_func(move |area, cr, w, h| {
            draw_history(area, cr, w as f64, h as f64, &shared.borrow(), span, &x_labels);
        });
        HistoryGraph { area, data }
    }

    pub fn set(&self, series: Vec<(Tone, Points)>, tests: Vec<(f64, f64)>, limit: Option<f64>) {
        *self.data.borrow_mut() = GraphData { series, tests, limit };
        self.area.queue_draw();
    }
}

/// x labels for a span of minutes: "−30 min", "−20 min", "−10 min", "Now".
pub fn minute_labels(steps: &[u32]) -> Vec<String> {
    let mut labels: Vec<String> = steps.iter().map(|m| crate::i18n::trf("−{} min", &[m])).collect();
    labels.push(tr("Now").to_string());
    labels
}

fn draw_history(area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64, data: &GraphData, span: f64, x_labels: &[String]) {
    let fg = named_color(area, "window_fg_color", gdk::RGBA::new(0.9, 0.9, 0.9, 1.0));
    let (fr, fgr, fb) = rgb(&fg);
    let (ax, ay) = (PAD_L, PAD_T);
    let (aw, ah) = ((w - PAD_L - PAD_R).max(1.0), (h - PAD_T - PAD_B).max(1.0));
    cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);

    let values: Vec<f64> = data.series.iter().flat_map(|(_, p)| p.iter().filter(|(ago, _)| *ago <= span).filter_map(|(_, v)| *v)).collect();
    // Whole tens with some room above and below, at least 30 °C tall, and
    // tall enough to show the limit.
    let (min, max) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let (min, max) = if min.is_finite() { (min, max) } else { (35.0, 55.0) };
    let lo = ((min - 2.0) / 10.0).floor() * 10.0;
    let top = data.limit.map_or(max + 2.0, |l| (max + 2.0).max(l + 2.0));
    let hi = ((top / 10.0).ceil() * 10.0).max(lo + 30.0);
    let px = |ago: f64, v: f64| (ax + (1.0 - ago / span) * aw, ay + ah - (v - lo) / (hi - lo) * ah);

    // Load tests as a band behind everything.
    let band = named_color(area, "stress_band", gdk::RGBA::new(0.94, 0.64, 0.0, 0.07));
    cr.set_source_rgba(band.red() as f64, band.green() as f64, band.blue() as f64, band.alpha() as f64);
    for &(from, to) in &data.tests {
        if to > span {
            continue;
        }
        let (x0, x1) = (px(from.min(span), 0.0).0, px(to, 0.0).0);
        cr.rectangle(x0, ay, (x1 - x0).max(2.0), ah);
    }
    let _ = cr.fill();

    // Grid and axis labels.
    cr.set_line_width(1.0);
    let mut v = lo;
    while v <= hi + 0.01 {
        let y = px(span, v).1;
        cr.set_source_rgba(fr, fgr, fb, 0.08);
        cr.move_to(ax, y.round() + 0.5);
        cr.line_to(ax + aw, y.round() + 0.5);
        let _ = cr.stroke();
        cr.set_source_rgba(fr, fgr, fb, 0.6);
        let text = format!("{v:.0} °C");
        let width = cr.text_extents(&text).map_or(0.0, |e| e.width());
        cr.move_to(ax - width - 8.0, y + 4.0);
        let _ = cr.show_text(&text);
        v += 10.0;
    }
    let n = x_labels.len().max(2) as f64 - 1.0;
    for (i, text) in x_labels.iter().enumerate() {
        let x = ax + i as f64 * aw / n;
        let last = i + 1 == x_labels.len();
        if i > 0 && !last {
            cr.set_source_rgba(fr, fgr, fb, 0.08);
            cr.set_dash(&[3.0, 4.0], 0.0);
            cr.move_to(x.round() + 0.5, ay);
            cr.line_to(x.round() + 0.5, ay + ah);
            let _ = cr.stroke();
            cr.set_dash(&[], 0.0);
        }
        let width = cr.text_extents(text).map_or(0.0, |e| e.width());
        let tx = if i == 0 { ax } else if last { ax + aw - width } else { x - width / 2.0 };
        cr.set_source_rgba(fr, fgr, fb, 0.6);
        cr.move_to(tx, ay + ah + 18.0);
        let _ = cr.show_text(text);
    }

    // The limit as a dashed red line with its value.
    if let Some(limit) = data.limit.filter(|l| (lo..=hi).contains(l)) {
        let red = named_color(area, "stress_emergency", gdk::RGBA::new(0.88, 0.1, 0.14, 1.0));
        let (r, g, b) = rgb(&red);
        let y = px(span, limit).1;
        cr.set_source_rgba(r, g, b, 0.9);
        cr.set_line_width(1.5);
        cr.set_dash(&[5.0, 4.0], 0.0);
        cr.move_to(ax, y);
        cr.line_to(ax + aw, y);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        let text = crate::i18n::trf("Stops at {}", &[&format!("{limit:.0} °C")]);
        let width = cr.text_extents(&text).map_or(0.0, |e| e.width());
        cr.move_to(ax + aw - width - 4.0, y - 6.0);
        let _ = cr.show_text(&text);
    }

    // The lines.
    for (tone, points) in &data.series {
        let color = named_color(area, tone.color_name(), GREY);
        let shown: Vec<(f64, Option<f64>)> = points.iter().filter(|(ago, _)| *ago <= span).copied().collect();
        let runs = runs(&shown, px);
        draw_runs(cr, &runs, &color, ay, ay + ah, 0.22);
        if let Some(&(ago, Some(v))) = shown.last() {
            end_dot(cr, px(ago, v), &color, 3.5);
        }
    }
}

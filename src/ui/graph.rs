//! Reading cards: a title, the current value in large type and a graph of
//! the last 30 minutes with its lowest and highest value, as in Vifte. Until
//! there are 30 minutes of history the graph spans what there is (at least
//! two minutes), so a fresh start is not squeezed against the right edge.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk};

use std::time::Duration;

use crate::history::KEEP;
use crate::i18n::trf;
use crate::sensors::format;

/// Readings as (seconds ago, value), oldest first.
pub type Points = Vec<(f64, Option<f64>)>;

const MIN_SPAN: Duration = Duration::from_secs(120);

#[derive(Default)]
struct Data {
    points: Points,
    /// When a load test ran, as (started, ended) in seconds ago.
    tests: Vec<(f64, f64)>,
    /// Seconds shown across the width.
    span: f64,
}

/// How the vertical axis is chosen.
#[derive(Clone, Copy)]
pub enum Scale {
    /// Always 0–100.
    Percent,
    /// Around the values, at least 10 degrees tall.
    Temperature,
    /// From zero up to a bit above the highest value.
    Power,
}

pub struct Card {
    pub widget: gtk::Box,
    value: gtk::Label,
    range: gtk::Label,
    area: gtk::DrawingArea,
    data: Rc<RefCell<Data>>,
    unit: &'static str,
    decimals: usize,
    prefix: &'static str,
}

impl Card {
    /// `prefix` goes before the value, e.g. "≈ " for estimates.
    pub fn new(title: &str, unit: &'static str, decimals: usize, scale: Scale, prefix: &'static str) -> Card {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["card", "reading-card"])
            .build();
        widget.append(
            &gtk::Label::builder()
                .label(title)
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["heading"])
                .build(),
        );
        let value = gtk::Label::builder().label("–").halign(gtk::Align::Start).css_classes(["reading-value"]).build();
        widget.append(&value);
        let data: Rc<RefCell<Data>> = Rc::default();
        let area = gtk::DrawingArea::builder().content_height(56).hexpand(true).build();
        let shared = data.clone();
        area.set_draw_func(move |area, cr, w, h| {
            let data = shared.borrow();
            draw(area, cr, w as f64, h as f64, &data, scale)
        });
        widget.append(&area);
        let range = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .css_classes(["caption", "dim-label", "reading"])
            .build();
        widget.append(&range);
        Card { widget, value, range, area, data, unit, decimals, prefix }
    }

    pub fn set(&self, current: Option<f64>, points: Points, tests: Vec<(f64, f64)>) {
        self.value.set_label(&match current {
            Some(_) => format!("{}{}", self.prefix, format(current, self.unit, self.decimals)),
            None => "–".into(),
        });
        let values = points.iter().filter_map(|(_, v)| *v);
        let (lo, hi) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let oldest = points.first().map_or(0.0, |(ago, _)| *ago);
        let span = oldest.clamp(MIN_SPAN.as_secs_f64(), KEEP.as_secs_f64());
        let minutes = (oldest / 60.0).ceil().clamp(1.0, KEEP.as_secs_f64() / 60.0);
        self.range.set_label(&if lo.is_finite() {
            trf(
                "Last {} min: {} – {}",
                &[&minutes, &format(Some(lo), "", self.decimals).trim_end(), &format(Some(hi), self.unit, self.decimals)],
            )
        } else {
            String::new()
        });
        *self.data.borrow_mut() = Data { points, tests, span };
        self.area.queue_draw();
    }
}

#[allow(deprecated)] // lookup_color has no replacement that works with libadwaita 1.5.
fn named_color(widget: &gtk::DrawingArea, name: &str, fallback: gdk::RGBA) -> gdk::RGBA {
    widget.style_context().lookup_color(name).unwrap_or(fallback)
}

fn draw(area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64, data: &Data, scale: Scale) {
    let fg = area.color();
    let span = data.span.max(1.0);
    let points = &data.points;

    // Load tests as a tinted band behind the graph.
    let warning = named_color(area, "warning_color", gdk::RGBA::new(0.9, 0.6, 0.1, 1.0));
    cr.set_source_rgba(warning.red() as f64, warning.green() as f64, warning.blue() as f64, 0.12);
    for &(from, to) in &data.tests {
        let (x0, x1) = (w * (1.0 - from.min(span) / span), w * (1.0 - to / span));
        cr.rectangle(x0, 0.0, (x1 - x0).max(1.0), h);
    }
    let _ = cr.fill();

    // Faint guides at whole minutes: every minute, 5 or 10 minutes.
    let step = if span <= 300.0 { 60.0 } else if span <= 900.0 { 300.0 } else { 600.0 };
    cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.08);
    cr.set_line_width(1.0);
    let mut ago = step;
    while ago < span {
        let x = (w * (1.0 - ago / span)).round() + 0.5;
        cr.move_to(x, 0.0);
        cr.line_to(x, h);
        ago += step;
    }
    let _ = cr.stroke();

    let values: Vec<f64> = points.iter().filter_map(|(_, v)| *v).collect();
    if values.is_empty() {
        return;
    }
    let (lo, hi) = values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let (lo, hi) = match scale {
        Scale::Percent => (0.0, 100.0),
        Scale::Power => (0.0, (hi * 1.15).max(10.0)),
        Scale::Temperature => {
            let mid = (lo + hi) / 2.0;
            let span = (hi - lo + 4.0).max(10.0);
            (mid - span / 2.0, mid + span / 2.0)
        }
    };
    let xy = |ago: f64, v: f64| (w * (1.0 - ago / span), 2.0 + (h - 4.0) * (1.0 - (v - lo) / (hi - lo)));

    // Split into runs at missing readings and at gaps (the computer slept).
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut last_ago = f64::INFINITY;
    for &(ago, value) in points.iter() {
        match value {
            Some(v) if last_ago - ago < 3.0 && !runs.is_empty() => runs.last_mut().unwrap().push(xy(ago, v)),
            Some(v) => runs.push(vec![xy(ago, v)]),
            None => last_ago = f64::INFINITY,
        }
        if value.is_some() {
            last_ago = ago;
        }
    }

    let accent = named_color(area, "accent_color", gdk::RGBA::new(0.2, 0.5, 0.9, 1.0));
    let (r, g, b) = (accent.red() as f64, accent.green() as f64, accent.blue() as f64);
    for run in runs {
        let (first, last) = (run[0], run[run.len() - 1]);
        cr.move_to(first.0, h);
        for &(x, y) in &run {
            cr.line_to(x, y);
        }
        cr.line_to(last.0, h);
        cr.close_path();
        cr.set_source_rgba(r, g, b, 0.15);
        let _ = cr.fill();
        cr.move_to(first.0, first.1);
        for &(x, y) in &run[1..] {
            cr.line_to(x, y);
        }
        if run.len() == 1 {
            cr.line_to(first.0 + 1.0, first.1);
        }
        cr.set_source_rgba(r, g, b, 1.0);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
    }
}

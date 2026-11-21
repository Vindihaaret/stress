//! The pages for the hardware – Processor, Graphics card and Power supply –
//! as grouped lists in Vifte's style: one row per reading with a sparkline
//! of the last two minutes, and the temperature of the last 30 minutes above.

use gtk::prelude::*;

use super::graph::{self, Chip, Points, Sparkline, Tone, SPARK_SPAN};
use crate::i18n::{tr, trf};
use crate::look::temperature_status;
use crate::sensors::format;

/// A page: the content in a clamp, scrolled.
pub fn page(content: &gtk::Box) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::builder().maximum_size(1400).tightening_threshold(1400).child(content).build();
    gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build()
}

pub fn page_box() -> gtk::Box {
    gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(16).margin_top(4).margin_bottom(24).margin_start(12).margin_end(12).build()
}

/// A group: its heading (with an icon surface and a note on the right) and
/// an empty list under it.
pub fn group(content: &gtk::Box, title: &str, icon: Option<(&[&str], &str)>, note: &str) -> gtk::ListBox {
    let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(9).build();
    let head = gtk::Box::builder().spacing(9).margin_start(4).margin_end(4).build();
    if let Some((names, kind)) = icon {
        head.append(&graph::icon_surface(names, kind, true));
    }
    head.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).tooltip_text(title).css_classes(["stress-group-label"]).build());
    if !note.is_empty() {
        head.append(&gtk::Label::builder().label(note).ellipsize(gtk::pango::EllipsizeMode::End).tooltip_text(note).css_classes(["caption", "dim-label"]).build());
    }
    section.append(&head);
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list", "stress-list"]).build();
    section.append(&list);
    content.append(&section);
    list
}

/// A small line of text under a group.
pub fn foot(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).wrap(true).xalign(0.0).margin_start(4).margin_end(4).css_classes(["caption", "dim-label", "stress-foot"]).build()
}

/// A row with an icon and a note, for a device that cannot be used.
pub fn note_row(text: &str) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&graph::image(graph::ICON_INFO, 16));
    content.append(&gtk::Label::builder().label(text).xalign(0.0).wrap(true).hexpand(true).css_classes(["dim-label"]).build());
    gtk::ListBoxRow::builder().activatable(false).focusable(false).css_classes(["stress-row"]).child(&content).build()
}

/// A row with a title on the left and its value on the right.
pub fn detail_row(list: &gtk::ListBox, title: &str) -> gtk::Label {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&gtk::Label::builder().label(title).xalign(0.0).hexpand(true).build());
    let value = gtk::Label::builder().xalign(1.0).css_classes(["heading", "reading"]).build();
    content.append(&value);
    list.append(&gtk::ListBoxRow::builder().activatable(false).focusable(false).css_classes(["stress-detail-row"]).child(&content).build());
    value
}

/// A card with the temperature of the last 30 minutes.
pub fn history_card(content: &gtk::Box, legend: &[(&str, &str)]) -> graph::HistoryGraph {
    let card = graph::card(8, "stress-history");
    card.append(&graph::graph_heading(tr("Temperature"), tr("last 30 minutes"), legend));
    let history = graph::HistoryGraph::new(180, crate::history::KEEP.as_secs_f64(), graph::minute_labels(&[30, 20, 10]));
    card.append(&history.area);
    content.append(&card);
    history
}

/// One reading: icon, name with its range over the last two minutes,
/// a sparkline and the value; temperatures show Warm or Hot when they are.
pub struct ReadingRow {
    pub row: gtk::ListBoxRow,
    value: gtk::Label,
    sub: gtk::Label,
    spark: Sparkline,
    chip: Option<Chip>,
    unit: &'static str,
    decimals: usize,
    detail: String,
}

impl ReadingRow {
    pub fn new(list: &gtk::ListBox, title: &str, icon: &[&str], tone: Tone, unit: &'static str, detail: &str) -> ReadingRow {
        let temperature = unit == "°C";
        let content = gtk::Box::builder().spacing(14).build();
        let icon = graph::image(icon, 16);
        icon.set_valign(gtk::Align::Center);
        icon.set_css_classes(&["stress-icon-surface", "row"]);
        content.append(&icon);
        let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
        texts.append(&gtk::Label::builder().label(title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-row-title"]).build());
        let sub = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label", "stress-sub"]).build();
        texts.append(&sub);
        content.append(&texts);
        let chip = temperature.then(Chip::new);
        if let Some(chip) = &chip {
            content.append(&chip.widget);
        }
        let min_span = match unit {
            "°C" => 4.0,
            "%" => 10.0,
            _ => 10.0,
        };
        let spark = Sparkline::new(tone, 120, 32, false, min_span);
        content.append(&spark.area);
        let value = gtk::Label::builder().xalign(1.0).width_chars(8).css_classes(["stress-value"]).build();
        content.append(&value);
        let row = gtk::ListBoxRow::builder().activatable(false).focusable(false).css_classes(["stress-row"]).child(&content).build();
        list.append(&row);
        ReadingRow { row, value, sub, spark, chip, unit, decimals: usize::from(temperature), detail: detail.to_owned() }
    }

    pub fn set(&self, points: &Points) {
        let current = points.last().and_then(|(_, v)| *v);
        // Translators: the decimal separator.
        self.value.set_label(&format(current, self.unit, self.decimals).replace('.', tr(".")));
        if let Some(chip) = &self.chip {
            let status = current.map(temperature_status).filter(|(level, _)| *level != "calm");
            chip.widget.set_visible(status.is_some());
            if let Some((level, word)) = status {
                chip.set(level, tr(word));
            }
        }
        let recent = points.iter().filter(|(ago, _)| *ago <= SPARK_SPAN).filter_map(|(_, v)| *v);
        let (lo, hi) = recent.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
        let mut parts = Vec::new();
        if lo.is_finite() {
            let oldest = points.iter().find(|(ago, _)| *ago <= SPARK_SPAN).map_or(0.0, |(ago, _)| *ago);
            let minutes = (oldest / 60.0).ceil().clamp(1.0, SPARK_SPAN / 60.0);
            parts.push(trf("Last {} min: {} – {}", &[&minutes, &format(Some(lo), "", 0).trim_end(), &format(Some(hi), self.unit, 0)]));
        }
        if !self.detail.is_empty() {
            parts.push(self.detail.clone());
        }
        self.sub.set_label(&parts.join(" · "));
        self.spark.set(points);
    }
}

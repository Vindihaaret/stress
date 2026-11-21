//! The Overview page, as in Vifte: four cards at the top (processor and
//! graphics card temperature, power, the load test), the temperature of the
//! last 30 minutes with load tests and the limit, and the cards Power and
//! Protection at the bottom.

use gtk::prelude::*;

use super::graph::{self, BigValue, Chip, HistoryGraph, Sparkline, Tone};
use crate::i18n::tr;
use crate::look;

/// A temperature card: icon and name, status, large number with a
/// sparkline, and the device's name under it.
pub struct TempTile {
    pub widget: gtk::Box,
    pub title: gtk::Label,
    chip: Chip,
    value: BigValue,
    pub spark: Sparkline,
    sub: gtk::Label,
}

impl TempTile {
    fn new(title: &str, icon: &[&str], tone: Tone) -> TempTile {
        let widget = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).hexpand(true).css_classes(["card", "stress-card", "stress-tile"]).build();
        let head = gtk::Box::builder().spacing(8).build();
        head.append(&graph::icon_surface(icon, tone.class(), false));
        let title = gtk::Label::builder().label(title).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).tooltip_text(title).css_classes(["heading"]).build();
        head.append(&title);
        let chip = Chip::new();
        head.append(&chip.widget);
        widget.append(&head);
        let body = gtk::Box::builder().spacing(10).build();
        let value = BigValue::new("stress-big");
        value.widget.set_hexpand(true);
        body.append(&value.widget);
        let spark = Sparkline::new(tone, 96, 34, true, 4.0);
        spark.area.set_valign(gtk::Align::End);
        body.append(&spark.area);
        widget.append(&body);
        // A small natural width, so four cards fit on one line; the text is
        // only cut when there is no room.
        let sub = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(12).css_classes(["caption", "dim-label"]).build();
        widget.append(&sub);
        TempTile { widget, title, chip, value, spark, sub }
    }

    /// `value` as text ("41.8 °C"), with the status chip for a temperature.
    pub fn set(&self, value: &str, temperature: Option<f64>, sub: &str) {
        self.value.set(value);
        self.chip.widget.set_visible(temperature.is_some());
        if let Some(t) = temperature {
            let (level, word) = look::temperature_status(t);
            self.chip.set(level, tr(word));
        }
        self.sub.set_label(sub);
        self.sub.set_tooltip_text(Some(sub));
    }
}

/// The load test card: time run, progress and the chosen load.
pub struct TestTile {
    pub widget: gtk::Box,
    pub chip: Chip,
    pub value: BigValue,
    pub progress: gtk::ProgressBar,
    pub sub: gtk::Label,
}

/// A line in the Power card: icon, name and what it does, its share of the
/// total as a bar, and the watts.
pub struct PowerRow {
    pub widget: gtk::Box,
    pub sub: gtk::Label,
    pub bar: gtk::LevelBar,
    pub value: gtk::Label,
}

impl PowerRow {
    fn new(title: &str, icon: &[&str], tone: Tone) -> PowerRow {
        let widget = gtk::Box::builder().spacing(12).css_classes(["stress-power-row"]).build();
        widget.append(&graph::icon_surface(icon, "neutral", false));
        let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
        texts.append(&gtk::Label::builder().label(title).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build());
        let sub = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label", "stress-sub"]).build();
        texts.append(&sub);
        widget.append(&texts);
        let bar = gtk::LevelBar::builder().min_value(0.0).max_value(1.0).valign(gtk::Align::Center).width_request(100).css_classes([tone.class()]).build();
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        widget.append(&bar);
        let value = gtk::Label::builder().xalign(1.0).width_chars(6).css_classes(["heading", "stress-sub"]).build();
        widget.append(&value);
        PowerRow { widget, sub, bar, value }
    }
}

pub struct Overview {
    pub page: gtk::ScrolledWindow,
    /// The four cards in two pairs; a breakpoint turns these.
    pub cards: gtk::Box,
    pub bottom: gtk::Box,
    pub cpu: TempTile,
    pub gpu: Option<TempTile>,
    pub power: TempTile,
    pub test: TestTile,
    pub test_click: gtk::GestureClick,
    pub history: HistoryGraph,
    pub power_link: gtk::Button,
    pub power_total: BigValue,
    pub power_total_sub: gtk::Label,
    pub power_cpu: PowerRow,
    pub power_gpu: Option<PowerRow>,
    pub power_rest: Option<PowerRow>,
    pub safety_link: gtk::Button,
    pub safety_icon: gtk::Image,
    pub safety_title: gtk::Label,
    pub safety_sub: gtk::Label,
    pub longest: gtk::SpinButton,
    pub battery: gtk::Switch,
    pub vifte_sub: gtk::Label,
    pub vifte_icon: gtk::Image,
}

/// A row in the Protection card: title and a line under it, and a control.
fn safety_row(rows: &gtk::Box, title: &str, control: &impl IsA<gtk::Widget>) -> gtk::Label {
    let row = gtk::Box::builder().spacing(12).css_classes(["stress-overview-row"]).build();
    let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
    texts.append(&gtk::Label::builder().label(title).xalign(0.0).css_classes(["heading"]).build());
    let sub = gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["caption", "dim-label"]).build();
    texts.append(&sub);
    row.append(&texts);
    control.set_valign(gtk::Align::Center);
    row.append(control);
    rows.append(&row);
    sub
}

impl Overview {
    /// `gpu` is the name of the first graphics card that can be used;
    /// `rest` says whether the power supply can tell the rest of the computer.
    pub fn new(gpu: Option<&str>, psu: bool, rest: bool, longest: (f64, f64)) -> Overview {
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(14).margin_top(4).margin_bottom(24).margin_start(12).margin_end(12).build();

        let cpu = TempTile::new(tr("Processor"), graph::ICON_CPU, Tone::Cpu);
        let gpu_tile = gpu.map(|_| TempTile::new(tr("Graphics card"), graph::ICON_GPU, Tone::Gpu));
        let power = TempTile::new("", graph::ICON_PSU, Tone::Neutral);

        let test_widget = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).hexpand(true).css_classes(["card", "stress-card", "stress-tile"]).build();
        let head = gtk::Box::builder().spacing(8).build();
        head.append(&graph::icon_surface(graph::ICON_TEST, "accent", false));
        head.append(&gtk::Label::builder().label(tr("Load Test")).xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build());
        let test_chip = Chip::new();
        head.append(&test_chip.widget);
        test_widget.append(&head);
        let test_value = BigValue::new("stress-big");
        test_value.widget.set_halign(gtk::Align::Start);
        test_widget.append(&test_value.widget);
        let test_progress = gtk::ProgressBar::builder().css_classes(["stress-progress", "thin"]).build();
        test_widget.append(&test_progress);
        let test_sub = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(12).css_classes(["caption", "dim-label", "stress-sub"]).build();
        test_widget.append(&test_sub);
        let test_click = gtk::GestureClick::new();
        test_widget.add_controller(test_click.clone());
        test_widget.set_cursor_from_name(Some("pointer"));
        let test = TestTile { widget: test_widget, chip: test_chip, value: test_value, progress: test_progress, sub: test_sub };

        // Four in a row; in narrow windows two and two.
        let cards = gtk::Box::builder().spacing(14).homogeneous(true).build();
        let first = gtk::Box::builder().spacing(14).homogeneous(true).build();
        first.append(&cpu.widget);
        match &gpu_tile {
            Some(tile) => first.append(&tile.widget),
            None => first.append(&power.widget),
        }
        let second = gtk::Box::builder().spacing(14).homogeneous(true).build();
        if gpu_tile.is_some() {
            second.append(&power.widget);
        }
        second.append(&test.widget);
        cards.append(&first);
        cards.append(&second);
        content.append(&cards);

        // The temperature of the last 30 minutes.
        let history_card = graph::card(8, "stress-history");
        let mut legend = vec![("cpu", tr("Processor"))];
        if gpu.is_some() {
            legend.push(("gpu", tr("Graphics card")));
        }
        legend.push(("band", tr("Test")));
        legend.push(("limit", tr("Limit")));
        history_card.append(&graph::graph_heading(tr("Temperature"), tr("last 30 minutes"), &legend));
        let history = HistoryGraph::new(190, crate::history::KEEP.as_secs_f64(), graph::minute_labels(&[30, 20, 10]));
        history_card.append(&history.area);
        content.append(&history_card);

        // At the bottom: Power and Protection.
        let bottom = gtk::Box::builder().spacing(14).homogeneous(true).build();
        let power_card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).css_classes(["card", "stress-card", "stress-bottom-card"]).build();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.append(&gtk::Label::builder().label(tr("Power")).xalign(0.0).hexpand(true).css_classes(["heading"]).build());
        let power_link = graph::link_button(tr("Power supply"));
        power_link.set_visible(psu);
        head.append(&power_link);
        power_card.append(&head);
        let summary = gtk::Box::builder().spacing(8).css_classes(["stress-summary"]).build();
        let power_total = BigValue::new("stress-big");
        power_total.widget.set_valign(gtk::Align::Center);
        summary.append(&power_total.widget);
        let power_total_sub = gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).valign(gtk::Align::Center).css_classes(["caption", "dim-label", "stress-sub"]).build();
        summary.append(&power_total_sub);
        power_card.append(&summary);
        let power_rows = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["stress-overview-rows"]).build();
        let power_cpu = PowerRow::new(tr("Processor"), graph::ICON_CPU, Tone::Cpu);
        power_rows.append(&power_cpu.widget);
        let power_gpu = gpu.map(|name| {
            let row = PowerRow::new(name, graph::ICON_GPU, Tone::Gpu);
            power_rows.append(&row.widget);
            row
        });
        let power_rest = rest.then(|| {
            let row = PowerRow::new(tr("Rest of the computer"), graph::ICON_REST, Tone::Neutral);
            row.sub.set_label(tr("Motherboard, memory, disks and fans"));
            power_rows.append(&row.widget);
            row
        });
        power_card.append(&power_rows);
        bottom.append(&power_card);

        let safety_card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).css_classes(["card", "stress-card", "stress-bottom-card"]).build();
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        head.append(&gtk::Label::builder().label(tr("Protection")).xalign(0.0).hexpand(true).css_classes(["heading"]).build());
        let safety_link = graph::link_button(tr("Preferences"));
        head.append(&safety_link);
        safety_card.append(&head);
        let summary = gtk::Box::builder().spacing(10).css_classes(["stress-summary"]).build();
        let safety_icon = gtk::Image::builder().icon_name("security-high-symbolic").valign(gtk::Align::Center).css_classes(["stress-status-icon", "calm"]).build();
        summary.append(&safety_icon);
        let texts = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
        let safety_title = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build();
        let safety_sub = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["caption", "dim-label", "stress-sub"]).build();
        texts.append(&safety_title);
        texts.append(&safety_sub);
        summary.append(&texts);
        safety_card.append(&summary);
        let rows = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["stress-overview-rows"]).build();
        let longest_button = gtk::SpinButton::with_range(longest.0, longest.1, 1.0);
        let longest_sub = safety_row(&rows, tr("Longest test"), &longest_button);
        longest_sub.set_label(tr("Minutes. Also applies to tests started without a duration."));
        let battery = gtk::Switch::new();
        let battery_sub = safety_row(&rows, tr("Stop when running on battery"), &battery);
        battery_sub.set_label(tr("A test stops when the power goes"));
        let vifte_icon = gtk::Image::builder().icon_name("emblem-ok-symbolic").build();
        let vifte_sub = safety_row(&rows, "Vifte", &vifte_icon);
        safety_card.append(&rows);
        bottom.append(&safety_card);
        content.append(&bottom);

        let clamp = adw::Clamp::builder().maximum_size(1400).tightening_threshold(1400).child(&content).build();
        let page = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&clamp).build();
        Overview {
            page,
            cards,
            bottom,
            cpu,
            gpu: gpu_tile,
            power,
            test,
            test_click,
            history,
            power_link,
            power_total,
            power_total_sub,
            power_cpu,
            power_gpu,
            power_rest,
            safety_link,
            safety_icon,
            safety_title,
            safety_sub,
            longest: longest_button,
            battery,
            vifte_sub,
            vifte_icon,
        }
    }
}

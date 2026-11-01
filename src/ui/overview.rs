//! The row at the top of the window: the two temperatures, power use and
//! whether a test has room to run.

use gtk::prelude::*;

use super::graph::{tile, Chip};
use crate::i18n::{tr, trf};
use crate::look;
use crate::sensors::format;
use crate::vifte::VifteState;

/// What the safety tile needs to know.
pub struct Safety<'a> {
    /// (temperature, limit) per device.
    pub readings: &'a [(Option<f64>, f64)],
    pub vifte: Option<&'a VifteState>,
    /// The last test was stopped by a temperature limit.
    pub limit_reached: bool,
}

pub struct Overview {
    pub grid: gtk::Grid,
    cpu: gtk::Widget,
    gpu: Option<gtk::Widget>,
    power: gtk::Box,
    safety: gtk::Box,
    power_title: gtk::Label,
    power_value: gtk::Label,
    power_sub: gtk::Label,
    safety_value: gtk::Label,
    safety_sub: gtk::Label,
    safety_chip: Chip,
}

fn title_label(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).halign(gtk::Align::Start).hexpand(true).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-tile-title"]).build()
}

fn sub_label() -> gtk::Label {
    gtk::Label::builder().halign(gtk::Align::Start).valign(gtk::Align::End).vexpand(true).xalign(0.0).wrap(true).css_classes(["stress-sub"]).build()
}

impl Overview {
    pub fn new(cpu: gtk::Widget, gpu: Option<gtk::Widget>) -> Overview {
        let power = tile();
        let power_title = title_label("");
        let power_value = gtk::Label::builder().label("–").halign(gtk::Align::Start).css_classes(["stress-value"]).build();
        let power_sub = sub_label();
        power.append(&power_title);
        power.append(&power_value);
        power.append(&power_sub);

        let safety = tile();
        let heading = gtk::Box::builder().spacing(8).build();
        heading.append(&title_label(tr("Safety")));
        let safety_chip = Chip::new();
        safety_chip.widget.set_visible(false);
        heading.append(&safety_chip.widget);
        let safety_value = gtk::Label::builder().halign(gtk::Align::Start).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["stress-text-value"]).build();
        let safety_sub = sub_label();
        safety.append(&heading);
        safety.append(&safety_value);
        safety.append(&safety_sub);

        let grid = gtk::Grid::builder().column_homogeneous(true).row_homogeneous(true).column_spacing(12).row_spacing(12).build();
        let overview = Overview { grid, cpu, gpu, power, safety, power_title, power_value, power_sub, safety_value, safety_sub, safety_chip };
        overview.arrange(false);
        overview
    }

    /// One row, or two when the window is narrow.
    pub fn arrange(&self, narrow: bool) {
        let tiles: [Option<&gtk::Widget>; 4] = [Some(&self.cpu), self.gpu.as_ref(), Some(self.power.upcast_ref()), Some(self.safety.upcast_ref())];
        for widget in tiles.into_iter().flatten() {
            if widget.parent().is_some() {
                self.grid.remove(widget);
            }
        }
        let temperature_span = if self.gpu.is_some() { 3 } else { 6 };
        if narrow {
            match &self.gpu {
                Some(gpu) => {
                    self.grid.attach(&self.cpu, 0, 0, 5, 1);
                    self.grid.attach(gpu, 5, 0, 5, 1);
                }
                None => self.grid.attach(&self.cpu, 0, 0, 10, 1),
            }
            self.grid.attach(&self.power, 0, 1, 5, 1);
            self.grid.attach(&self.safety, 5, 1, 5, 1);
        } else {
            self.grid.attach(&self.cpu, 0, 0, temperature_span, 1);
            if let Some(gpu) = &self.gpu {
                self.grid.attach(gpu, 3, 0, 3, 1);
            }
            self.grid.attach(&self.power, 6, 0, 2, 1);
            self.grid.attach(&self.safety, 8, 0, 2, 1);
        }
    }

    /// `title`, the value and a small line under it.
    pub fn show_power(&self, title: &str, value: &str, sub: &str) {
        self.power_title.set_label(title);
        self.power_value.set_label(value);
        self.power_sub.set_label(sub);
    }

    pub fn set_power_tooltip(&self, text: Option<&str>) {
        self.power.set_tooltip_text(text);
    }

    pub fn show_safety(&self, safety: &Safety) {
        let vifte_problem = safety.vifte.is_some_and(|v| v.emergency_active || v.sensor_lost || v.paused);
        self.safety_value.set_label(if safety.limit_reached {
            tr("Limit reached")
        } else if vifte_problem {
            tr("Check Vifte")
        } else {
            tr("All good")
        });
        self.safety_chip.widget.set_visible(safety.limit_reached);
        if safety.limit_reached {
            self.safety_chip.set("hot", tr("Stopped"));
        }
        let mut lines = Vec::new();
        let mut second = Vec::new();
        if let Some((limit, left)) = look::nearest_limit(safety.readings) {
            lines.push(trf("Stops at {}", &[&format(Some(limit), "°C", 0)]));
            second.push(trf("{} left", &[&format(Some(left), "°C", 0)]));
        }
        if safety.vifte.is_some() {
            second.push(tr("Vifte is running").to_string());
        }
        if !second.is_empty() {
            lines.push(second.join(" · "));
        }
        self.safety_sub.set_label(&lines.join("\n"));
    }
}

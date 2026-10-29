//! Status icon (StatusNotifierItem): shown by KDE Plasma, and by GNOME with
//! the AppIndicator extension. While a test runs it shows the hottest loaded
//! temperature instead of the app icon. It runs on its own thread and talks to
//! the UI through a channel, so it outlives window rebuilds.

use gtk::{cairo, glib};
use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip};

use crate::i18n::tr;
use crate::APP_ID;

pub enum Command {
    Show,
    StartStop,
    Quit,
}

/// What the icon shows; the UI pushes changes with `Handle::update`.
#[derive(Clone, Default, PartialEq)]
pub struct State {
    pub running: bool,
    /// Hottest temperature among the loaded parts, shown in the icon while
    /// a test runs.
    pub hottest: Option<f64>,
    /// Readings for the tooltip, e.g. "Processor 55 °C · 84 W".
    pub summary: String,
}

pub struct StressTray {
    tx: async_channel::Sender<Command>,
    icon: String,
    pub state: State,
}

impl StressTray {
    fn send(&self, command: Command) {
        let _ = self.tx.send_blocking(command);
    }
}

impl ksni::Tray for StressTray {
    fn id(&self) -> String {
        "stresstest".into()
    }

    fn title(&self) -> String {
        "Stress".into()
    }

    fn icon_name(&self) -> String {
        // An icon name wins over pixmaps, so leave it out while a value shows.
        if self.state.running && self.state.hottest.is_some() { String::new() } else { self.icon.clone() }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        match (self.state.running, self.state.hottest) {
            (true, Some(t)) => [22, 32, 48].iter().filter_map(|&size| render_value(&format!("{t:.0}°"), size)).collect(),
            _ => Vec::new(),
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let title = if self.state.running { tr("Stress – test running") } else { "Stress" };
        ToolTip { title: title.into(), description: self.state.summary.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Command::Show);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: tr("Show Stress").into(),
                activate: Box::new(|this: &mut Self| this.send(Command::Show)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if self.state.running { tr("Stop the Test") } else { tr("Start a Test") }.into(),
                icon_name: if self.state.running { "media-playback-stop-symbolic" } else { "media-playback-start-symbolic" }.into(),
                activate: Box::new(|this: &mut Self| this.send(Command::StartStop)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: tr("Quit").into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|this: &mut Self| this.send(Command::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// White text on an orange rounded square, readable on light and dark panels.
/// StatusNotifierItem wants ARGB32 in network byte order.
fn render_value(text: &str, size: i32) -> Option<Icon> {
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        let s = size as f64;
        let r = s * 0.2;
        cr.new_sub_path();
        cr.arc(s - r, r, r, -std::f64::consts::FRAC_PI_2, 0.0);
        cr.arc(s - r, s - r, r, 0.0, std::f64::consts::FRAC_PI_2);
        cr.arc(r, s - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
        cr.arc(r, r, r, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
        cr.close_path();
        cr.set_source_rgb(0.90, 0.38, 0.0);
        cr.fill().ok()?;
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        // Shrink until the text fits with a small margin.
        let mut font = s * 0.6;
        cr.set_font_size(font);
        while cr.text_extents(text).ok()?.width() > s * 0.9 && font > 4.0 {
            font -= 0.5;
            cr.set_font_size(font);
        }
        let extents = cr.text_extents(text).ok()?;
        cr.move_to(
            (s - extents.width()) / 2.0 - extents.x_bearing(),
            (s - extents.height()) / 2.0 - extents.y_bearing(),
        );
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.show_text(text).ok()?;
    }
    surface.flush();
    let data = surface.data().ok()?;
    let mut argb = Vec::with_capacity(data.len());
    for pixel in data.as_chunks::<4>().0 {
        let value = u32::from_ne_bytes(*pixel);
        argb.extend_from_slice(&value.to_be_bytes());
    }
    Some(Icon { width: size, height: size, data: argb })
}

/// The installed symbolic app icon, or a stock icon when running from the
/// build directory.
fn icon_name() -> String {
    let name = format!("{APP_ID}-symbolic");
    let installed = [glib::user_data_dir(), "/usr/share".into(), "/usr/local/share".into()]
        .iter()
        .any(|dir| dir.join(format!("icons/hicolor/symbolic/apps/{name}.svg")).exists());
    if installed { name } else { "utilities-system-monitor-symbolic".into() }
}

/// Starts the status icon. Returns `None` when the desktop has no tray
/// (for example GNOME without the AppIndicator extension).
pub fn spawn(tx: async_channel::Sender<Command>) -> Option<Handle<StressTray>> {
    let tray = StressTray { tx, icon: icon_name(), state: State::default() };
    tray.spawn().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_value_icon() {
        let icon = render_value("72°", 32).unwrap();
        assert_eq!(icon.data.len(), 32 * 32 * 4);
        // A corner is transparent, the middle is opaque.
        assert_eq!(icon.data[0], 0);
        let middle = (16 * 32 + 16) * 4;
        assert_eq!(icon.data[middle], 255);
    }
}

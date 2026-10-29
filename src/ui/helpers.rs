//! Installing and removing the system helpers from the window: the offer to
//! show the processor's power use, their state in the preferences, and
//! removing them when Stress is uninstalled.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::App;
use crate::helpers::{self, Error, RAPL};
use crate::i18n::{tr, trf};
use crate::sensors::{rapl_access, RaplAccess};

impl App {
    /// The offer shown under the processor when its power use is hidden.
    pub(super) fn rapl_offer(self: &Rc<Self>) -> Option<gtk::ListBox> {
        if rapl_access() != RaplAccess::Denied {
            return None;
        }
        let row = adw::ActionRow::builder()
            .title(tr("Processor power is not shown"))
            .subtitle(tr("On this system only the administrator can read it."))
            .build();
        let allow = gtk::Button::builder().label(tr("Allow…")).valign(gtk::Align::Center).build();
        row.add_suffix(&allow);
        let weak = Rc::downgrade(self);
        allow.connect_clicked(move |_| {
            if let Some(app) = weak.upgrade() {
                app.ask_rapl();
            }
        });
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        list.append(&row);
        Some(list)
    }

    fn ask_rapl(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some(tr("Show the processor's power use?")),
            Some(tr("Stress installs a small system rule that lets programs read the processor's energy counters. Some systems keep them private because, in rare cases, they can be used to work out secrets such as encryption keys. You can remove the rule again in the preferences, and uninstalling Stress removes it too.")),
        );
        dialog.add_responses(&[("cancel", tr("Cancel")), ("allow", tr("Allow"))]);
        dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response != "allow" {
                return;
            }
            let Some(app) = weak.upgrade() else { return };
            let weak = Rc::downgrade(&app);
            helpers::install(&RAPL, move |result| {
                if let Some(app) = weak.upgrade() {
                    app.helper_changed(result, tr("The processor's power use is now shown."));
                }
            });
        });
        dialog.present(Some(&self.window));
    }

    /// Reports how installing or removing went, and rebuilds the window so
    /// cards and offers match the new state.
    fn helper_changed(self: &Rc<Self>, result: Result<(), Error>, success: &str) {
        match result {
            Ok(()) => {
                self.toast(success);
                // Give the sensor thread a moment to pick up the change.
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                    if let Some(app) = weak.upgrade() {
                        app.rebuild_window();
                    }
                });
            }
            Err(Error::Cancelled) => {}
            Err(Error::Failed(message)) => self.toast(&trf("The administrator task failed: {}", &[&message])),
        }
    }

    /// The helpers group in the preferences: what Stress has installed on
    /// the system, with a way to install or remove it.
    pub(super) fn helpers_group(self: &Rc<Self>, dialog: &adw::PreferencesDialog) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr("System Helpers"))
            .description(tr("Changes Stress can make to the system with the administrator password. Uninstalling Stress removes them."))
            .build();
        let row = adw::ActionRow::builder().title(tr("Processor power access")).build();
        let (subtitle, button) = match (RAPL.is_installed(), rapl_access()) {
            (true, _) => (tr("Installed by Stress"), Some((tr("Remove"), false))),
            (false, RaplAccess::Denied) => (tr("Not installed; the processor's power use is hidden"), Some((tr("Install"), true))),
            (false, RaplAccess::Readable) => (tr("Not needed on this computer"), None),
            (false, RaplAccess::Missing) => (tr("This processor does not report its power use"), None),
        };
        row.set_subtitle(subtitle);
        if let Some((label, install)) = button {
            let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
            row.add_suffix(&button);
            let weak = Rc::downgrade(self);
            let dialog = dialog.clone();
            button.connect_clicked(move |_| {
                dialog.close();
                let Some(app) = weak.upgrade() else { return };
                if install {
                    app.ask_rapl();
                    return;
                }
                let weak = Rc::downgrade(&app);
                helpers::remove(&[&RAPL], move |result| {
                    if let Some(app) = weak.upgrade() {
                        app.helper_changed(result, tr("Processor power access is removed."));
                    }
                });
            });
        }
        group.add(&row);
        group
    }

    /// Removes every installed helper, then runs `then` whatever the outcome,
    /// so a refused password does not stop the rest of the uninstall.
    pub(super) fn remove_helpers_then(self: &Rc<Self>, then: impl FnOnce(&Rc<App>) + 'static) {
        let installed = helpers::installed();
        if installed.is_empty() {
            then(self);
            return;
        }
        let weak = Rc::downgrade(self);
        helpers::remove(&installed, move |result| {
            let Some(app) = weak.upgrade() else { return };
            if let Err(Error::Failed(message)) = result {
                app.toast(&trf("Could not remove the system helpers: {}", &[&message]));
            }
            then(&app);
        });
    }
}

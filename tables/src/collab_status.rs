// SPDX-License-Identifier: GPL-3.0-or-later
//! The status button (RFC-0005 5.1): the only always-visible trace of a
//! session. An icon-only `GtkMenuButton` in the header bar's end box beside
//! the main menu, hidden entirely when no session exists. Colour is never the
//! only signal (docs/GNOME-GUIDELINES.md §8): the accessible name and the
//! tooltip say it in words, and so does the popover.

use gtk4::prelude::*;
use gtk4::{self as gtk};

/// What the button says about the session.
pub(crate) struct StatusModel {
    pub icon: &'static str,
    /// "Sharing: online, 2 people": the accessible name and the tooltip.
    pub summary: String,
    /// The people the session has heard of, one line each.
    pub people: String,
    /// "Last update from ed25519:… 12 s ago".
    pub last_update: Option<String>,
    /// Something the sidecar refused or lost, until the next one.
    pub problem: Option<String>,
}

pub(crate) struct Status {
    pub button: gtk::MenuButton,
    people: gtk::Label,
    last: gtk::Label,
    problem: gtk::Label,
}

fn caption(css: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder().xalign(0.0).wrap(true).max_width_chars(40).selectable(true).build();
    for class in css {
        label.add_css_class(class);
    }
    label.set_visible(false);
    label
}

impl Status {
    pub(crate) fn new() -> Self {
        let people = caption(&[]);
        let last = caption(&["dim-label"]);
        let problem = caption(&["error"]);
        let copy = gtk::Button::builder()
            .label(suite_common::i18n("_Copy Join Code"))
            .use_underline(true)
            .action_name("app.collab-copy-code")
            .build();
        let stop = gtk::Button::builder()
            .label(suite_common::i18n("_Stop Sharing"))
            .use_underline(true)
            .action_name("app.collab-leave")
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(12);
        body.set_margin_end(12);
        for w in [people.upcast_ref::<gtk::Widget>(), last.upcast_ref(), problem.upcast_ref(), copy.upcast_ref(), stop.upcast_ref()] {
            body.append(w);
        }
        let popover = gtk::Popover::builder().child(&body).build();
        for b in [&copy, &stop] {
            let p = popover.clone();
            b.connect_clicked(move |_| p.popdown());
        }
        let button = gtk::MenuButton::builder()
            .icon_name("network-transmit-receive-symbolic")
            .popover(&popover)
            .visible(false)
            .build();
        Status { button, people, last, problem }
    }

    /// Show the session as `model` says, or hide the button when there is none.
    pub(crate) fn update(&self, model: Option<&StatusModel>) {
        let Some(m) = model else {
            self.button.set_visible(false);
            return;
        };
        self.button.set_icon_name(m.icon);
        self.button.set_tooltip_text(Some(&m.summary));
        self.button.update_property(&[gtk::accessible::Property::Label(&m.summary)]);
        self.people.set_text(&m.people);
        self.people.set_visible(!m.people.is_empty());
        self.last.set_text(m.last_update.as_deref().unwrap_or(""));
        self.last.set_visible(m.last_update.is_some());
        self.problem.set_text(m.problem.as_deref().unwrap_or(""));
        self.problem.set_visible(m.problem.is_some());
        self.button.set_visible(true);
    }
}

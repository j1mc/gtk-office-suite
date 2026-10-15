// name_box.rs — the formula bar's name box: the active cell's reference,
// and where a reference is typed to jump to it.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Every refresh of the grid shows the active cell here. It must not do that
// while the user is typing a reference: GTK 4's `gtk_editable_set_text`
// replaces the buffer even when the text is unchanged, which drops the
// selection Ctrl+G made and puts the caret at the start, so a refresh landing
// between Ctrl+G and the typing turned "A1" into "A1A1" and the jump never
// happened. A column-menu button activated over AT-SPI does exactly that:
// GtkButton reports that activation's click 250 ms later, by when the user
// can have closed the menu and pressed Ctrl+G (TBL-006). While the focus is
// in the box its text is the user's; the active cell's reference comes back
// when they leave it — after a jump, after Escape.

use gtk4::{self as gtk, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

/// The name box entry and the reference it shows whenever it is not being
/// typed in.
#[derive(Clone)]
pub(crate) struct NameBox {
    entry: gtk::Entry,
    active: Rc<RefCell<String>>,
}

impl NameBox {
    pub(crate) fn new(entry: &gtk::Entry) -> Self {
        let active = Rc::new(RefCell::new(entry.text().to_string()));
        let focus = gtk::EventControllerFocus::new();
        let (weak, shown) = (entry.downgrade(), active.clone());
        focus.connect_leave(move |_| {
            if let Some(entry) = weak.upgrade() {
                show(&entry, &shown.borrow());
            }
        });
        entry.add_controller(focus);
        Self { entry: entry.clone(), active }
    }

    /// Show `reference`, the active cell's: now, or when the user leaves
    /// the box if they are typing in it.
    pub(crate) fn show(&self, reference: &str) {
        *self.active.borrow_mut() = reference.to_owned();
        show(&self.entry, reference);
    }
}

fn show(entry: &gtk::Entry, reference: &str) {
    if typing_in(entry) || entry.text().as_str() == reference {
        return;
    }
    entry.set_text(reference);
}

/// Whether the window's keyboard focus is in `entry` (its GtkText child
/// holds it, not the entry). A window keeps its focus widget while another
/// window is active, so switching away mid-reference is not leaving.
fn typing_in(entry: &gtk::Entry) -> bool {
    entry.root().and_then(|root| root.focus()).is_some_and(|focus| focus.is_ancestor(entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_leaves_a_reference_being_typed_alone_until_the_box_is_left() {
        suite_common::gtk_test::run(|| {
            let window = gtk::Window::new();
            let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let entry = gtk::Entry::new();
            let grid = gtk::DrawingArea::new();
            grid.set_focusable(true);
            column.append(&entry);
            column.append(&grid);
            window.set_child(Some(&column));
            let name_box = NameBox::new(&entry);
            name_box.show("A1");
            assert_eq!(entry.text().as_str(), "A1", "out of the box, it follows the active cell");

            // Ctrl+G: the box takes the focus with its text selected, so
            // typing replaces it.
            entry.grab_focus();
            entry.select_region(0, -1);
            name_box.show("A1");
            assert_eq!(entry.selection_bounds(), Some((0, 2)), "a refresh kept Ctrl+G's selection");

            entry.delete_selection();
            let mut at = 0;
            entry.insert_text("B", &mut at);
            entry.set_position(at);
            name_box.show("C3");
            assert_eq!(entry.text().as_str(), "B", "a refresh left the typing alone");
            assert_eq!(entry.position(), 1, "and the caret where the typing left it");

            grid.grab_focus();
            assert_eq!(entry.text().as_str(), "C3", "leaving the box shows the active cell again");
            window.destroy();
        });
    }
}

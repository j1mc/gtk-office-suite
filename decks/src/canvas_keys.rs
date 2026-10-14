//! canvas_keys.rs — the editor's keyboard and clipboard, and editing a
//! text box in place: split out of window.rs (which keeps the widgets and
//! the file actions) to give it room for masters and layouts.
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! Pointer input on the canvas is canvas_input.rs.

use gtk4::{self as gtk, glib, prelude::*};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::canvas::{canvas_to_slide, hit_test_object};
use crate::sidebar::rebuild_slide_list;
use crate::transition::{TransitionState, TransitionType};
use decks_core::engine::{MasterSlide, Slide, SlideObject};
use decks_core::DecksController;

/// The editor state the handlers here share with the window.
#[derive(Clone, Copy)]
pub(crate) struct EditorHandles<'a> {
    pub window: &'a adw::ApplicationWindow,
    pub canvas: &'a gtk::DrawingArea,
    pub slide_list: &'a gtk::ListBox,
    pub slides: &'a Rc<RefCell<Vec<Slide>>>,
    pub masters: &'a Rc<RefCell<Vec<MasterSlide>>>,
    pub controller: &'a Rc<DecksController>,
    pub refresh_hud: &'a Rc<dyn Fn()>,
    pub transition: &'a Rc<RefCell<TransitionState>>,
}

/// Cross-app copy and paste (DESIGN-UI), in place text editing on a
/// double-click, and the editor's keys: navigation, delete, undo and redo.
pub(crate) fn register(h: EditorHandles, editor_host: &gtk::Overlay) {
    let EditorHandles { window, canvas, slide_list, slides, masters, controller, refresh_hud, transition } = h;
    let text_editing = register_text_editor(h, editor_host);
    // ── Cross-app clipboard (DESIGN-UI): Ctrl+C copies the selected
    // text box as a styled fragment; Ctrl+V pastes a fragment as a
    // new text box. Window-level capture, skipped while an entry or
    // the notes view has focus.
    {
        let ss = slides.clone();
        let cs = canvas.clone();
        let controller2 = controller.clone();
        let refresh = refresh_hud.clone();
        let win = window.clone();
        let text_editing = text_editing.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _code, mods| {
            let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if !ctrl || (keyval != gtk::gdk::Key::c && keyval != gtk::gdk::Key::v) {
                return glib::Propagation::Proceed;
            }
            // Text widgets keep their own clipboard behavior.
            if text_editing.get() || gtk::prelude::GtkWindowExt::focus(&win)
                .map(|w| w.is::<gtk::Text>() || w.is::<gtk::TextView>() || w.is::<gtk::Entry>())
                .unwrap_or(false)
            {
                return glib::Propagation::Proceed;
            }
            if keyval == gtk::gdk::Key::c {
                let slides = ss.borrow();
                let frag = controller2
                    .primary_object_index()
                    .zip(controller2.selected_slide_index())
                    .and_then(|(oi, idx)| slides.get(idx).and_then(|s| s.objects.get(oi)))
                    .and_then(decks_core::fragment::copy_object);
                if let Some(frag) = frag {
                    let provider = suite_common::clipboard::provider(
                        decks_core::fragment::MIME,
                        &frag.to_json(),
                        &frag.to_html(),
                        &frag.to_plain(),
                    );
                    let _ = cs.clipboard().set_content(Some(&provider));
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            // Paste
            let clipboard = cs.clipboard();
            if !suite_common::clipboard::offers(&clipboard, decks_core::fragment::MIME) {
                return glib::Propagation::Proceed;
            }
            let cs2 = cs.clone();
            let controller3 = controller2.clone();
            let refresh2 = refresh.clone();
            suite_common::clipboard::read_string(
                &clipboard,
                decks_core::fragment::MIME,
                move |json| {
                    if let Some(frag) = json
                        .as_deref()
                        .and_then(decks_core::fragment::Fragment::from_json)
                    {
                        let obj = decks_core::fragment::paste_as_text_box(&frag, 240.0, 200.0);
                        if let Some(slide_id) = controller3.selection().slide_id {
                            controller3.add_object(slide_id, obj);
                        }
                        cs2.queue_draw();
                        refresh2();
                    }
                },
            );
            glib::Propagation::Stop
        });
        window.add_controller(key);
    }


    // ── Keyboard: navigation, delete, undo/redo ─────────────────────
    {
        let w = window.clone();
        let sl = slide_list.clone();
        let ss = slides.clone();
        let cs = canvas.clone();
        let controller = controller.clone();
        let ts = transition.clone();
        let m = masters.clone();
        let refresh = refresh_hud.clone();
        let key = gtk::EventControllerKey::new();
        let text_editing = text_editing.clone();
        key.connect_key_pressed(move |_, keyval, _code, mods| {
            // Canvas commands stay suspended throughout the text session,
            // including while focus moves to the editor's buttons.
            if text_editing.get() {
                return glib::Propagation::Proceed;
            }
            // Ctrl+Z: undo
            if mods.contains(gtk::gdk::ModifierType::CONTROL_MASK) && keyval == gtk::gdk::Key::z {
                if controller.undo() {
                    controller.repair_selection();
                    cs.queue_draw();
                    // Snapshot instead of holding ss.borrow() across the
                    // call -- rebuild_slide_list()'s select_row() fires
                    // row-selected synchronously, which reaches ss.borrow_mut().
                    let slides = ss.borrow().clone();
                    rebuild_slide_list(&sl, &slides, &m.borrow(), controller.selected_slide_index().unwrap_or(0));
                }
                return glib::Propagation::Stop;
            }
            // Ctrl+Shift+Z: redo
            if mods.contains(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::SHIFT_MASK) && keyval == gtk::gdk::Key::z {
                if controller.redo() {
                    controller.repair_selection();
                    cs.queue_draw();
                    let slides = ss.borrow().clone();
                    rebuild_slide_list(&sl, &slides, &m.borrow(), controller.selected_slide_index().unwrap_or(0));
                }
                return glib::Propagation::Stop;
            }
            match keyval {
                gtk::gdk::Key::Escape => {
                    w.unfullscreen();
                    glib::Propagation::Stop
                }
                // Tab and Shift+Tab select the slide's objects in turn, so
                // the keyboard reaches what the pointer does: the inspector
                // follows the selection (PowerPoint and Keynote do the same).
                gtk::gdk::Key::Tab | gtk::gdk::Key::ISO_Left_Tab => {
                    let sel = controller.selection();
                    let Some(slide_id) = sel.slide_id else {
                        return glib::Propagation::Proceed;
                    };
                    let Some(idx) = controller.slide_index(slide_id) else {
                        return glib::Propagation::Proceed;
                    };
                    let count = ss.borrow().get(idx).map_or(0, |s| s.objects.len());
                    if count == 0 {
                        return glib::Propagation::Proceed;
                    }
                    let back = keyval == gtk::gdk::Key::ISO_Left_Tab || mods.contains(gtk::gdk::ModifierType::SHIFT_MASK);
                    let next = next_object(controller.primary_object_index(), count, back);
                    if let Some(object_id) = controller.object_id_at(idx, next) {
                        controller.select_objects(slide_id, &[object_id], Some(object_id), decks_core::controller::SelectionMode::Object);
                    }
                    refresh();
                    cs.queue_draw();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Left | gtk::gdk::Key::Up => {
                    if let Some(idx) = controller.selected_slide_index() {
                        if idx > 0 {
                            let sls = ss.borrow();
                            if idx < sls.len() && idx > 0 {
                                // Backwards, the slide being left plays
                                // its own transition in reverse.
                                TransitionState::start(&ts, TransitionType::of(sls[idx].transition),
                                    &sls[idx], &sls[idx - 1], &m.borrow(), &cs);
                            }
                            // Snapshot + drop before rebuild_slide_list(),
                            // whose select_row() reaches ss.borrow_mut().
                            let snap = sls.clone();
                            drop(sls);
                            if let Some(slide_id) = controller.slide_id_at(idx - 1) {
                                controller.select_slide(slide_id);
                            }
                            rebuild_slide_list(&sl, &snap, &m.borrow(), idx - 1);
                            cs.queue_draw();
                        }
                    }
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Right | gtk::gdk::Key::Down | gtk::gdk::Key::space => {
                    if let Some(idx) = controller.selected_slide_index() {
                        let slides = ss.borrow();
                        if idx + 1 < slides.len() {
                            TransitionState::start(&ts, TransitionType::of(slides[idx + 1].transition),
                                &slides[idx], &slides[idx + 1], &m.borrow(), &cs);
                            let snap = slides.clone();
                            drop(slides);
                            if let Some(slide_id) = controller.slide_id_at(idx + 1) {
                                controller.select_slide(slide_id);
                            }
                            rebuild_slide_list(&sl, &snap, &m.borrow(), idx + 1);
                            cs.queue_draw();
                        }
                    }
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Home => {
                    if let Some(slide_id) = controller.slide_id_at(0) {
                        controller.select_slide(slide_id);
                    }
                    let snap = ss.borrow().clone();
                    rebuild_slide_list(&sl, &snap, &m.borrow(), 0);
                    cs.queue_draw();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::End => {
                    let slides = ss.borrow();
                    if !slides.is_empty() {
                        let snap = slides.clone();
                        let last = snap.len() - 1;
                        drop(slides);
                        if let Some(slide_id) = controller.slide_id_at(last) {
                            controller.select_slide(slide_id);
                        }
                        rebuild_slide_list(&sl, &snap, &m.borrow(), last);
                        cs.queue_draw();
                    }
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Delete | gtk::gdk::Key::BackSpace => {
                    let sel = controller.selection();
                    if let (Some(slide_id), Some(object_id)) = (sel.slide_id, sel.primary_object_id) {
                        if controller.delete_object(slide_id, object_id) {
                            cs.queue_draw();
                        }
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        canvas.add_controller(key);
    }
}

struct TextEdit {
    slide_id: u64,
    object_id: u64,
    master: Option<u64>,
    original: String,
    bounds: (f64, f64, f64, f64),
}

/// A native multiline editor anchored to the actual canvas. Its host must
/// be outside CanvasArea: that widget exposes only virtual slide objects
/// as accessible children, hiding real child widgets from assistive tools.
/// Keep the popover alive across sessions (suite-common's surface-lifetime
/// rule); moving focus inside it is not a commit.
fn register_text_editor(h: EditorHandles, editor_host: &gtk::Overlay) -> Rc<Cell<bool>> {
    let popover = suite_common::popover::reused(editor_host, "slide-text-editor");
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_autohide(true);
    let text_view = gtk::TextView::new();
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(8);
    text_view.set_bottom_margin(8);
    text_view.update_property(&[
        gtk::accessible::Property::Label("Slide text editor"),
        gtk::accessible::Property::Description("Enter inserts a new line. Control+Enter applies changes. Escape cancels."),
    ]);
    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_child(Some(&text_view));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.append(&scrolled);
    let hint = gtk::Label::new(Some(&suite_common::i18n("Enter: new line · Ctrl+Enter: apply · Escape: cancel")));
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    hint.set_wrap(true);
    body.append(&hint);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label(&suite_common::i18n("Cancel"));
    let apply = gtk::Button::with_label(&suite_common::i18n("Apply"));
    apply.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    body.append(&buttons);
    popover.set_child(Some(&body));

    let session: Rc<RefCell<Option<TextEdit>>> = Rc::default();
    let editing = Rc::new(Cell::new(false));
    let cancelled = Rc::new(Cell::new(false));
    let restore_focus = Rc::new(Cell::new(false));
    for (button, discard) in [(cancel, true), (apply, false)] {
        let popover = popover.downgrade();
        let cancelled = cancelled.clone();
        let restore_focus = restore_focus.clone();
        button.connect_clicked(move |_| {
            cancelled.set(discard);
            restore_focus.set(true);
            if let Some(popover) = popover.upgrade() {
                popover.popdown();
            }
        });
    }
    {
        let weak_popover = popover.downgrade();
        let cancelled = cancelled.clone();
        let restore_focus = restore_focus.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _, mods| {
            let cancel = keyval == gtk::gdk::Key::Escape;
            let apply = matches!(keyval, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter)
                && mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            if !cancel && !apply {
                return glib::Propagation::Proceed;
            }
            cancelled.set(cancel);
            restore_focus.set(true);
            if let Some(popover) = weak_popover.upgrade() {
                popover.popdown();
            }
            glib::Propagation::Stop
        });
        popover.add_controller(key);
    }
    {
        let session = session.clone();
        let editing = editing.clone();
        let controller = h.controller.clone();
        let canvas = h.canvas.downgrade();
        let refresh = h.refresh_hud.clone();
        let buffer = text_view.buffer();
        popover.connect_closed(move |_| {
            editing.set(false);
            let edit = session.borrow_mut().take();
            let discard = cancelled.replace(false);
            if let Some(edit) = edit.filter(|_| !discard) {
                let new_text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
                if new_text != edit.original && controller.editing_master() == edit.master {
                    // Navigation, reordering and deletion can happen while a
                    // popover closes. Resolve the captured identities, never
                    // the current slide or stale object index.
                    let target = {
                        let slides = controller.slides.borrow();
                        slides.iter().find(|s| s.ids.slide == edit.slide_id).and_then(|slide| {
                            let oi = slide.ids.objects.iter().position(|id| *id == edit.object_id)?;
                            match slide.objects.get(oi)? {
                                SlideObject::TextBox { text, .. } if *text == edit.original => Some((edit.slide_id, edit.object_id)),
                                _ => None,
                            }
                        })
                    };
                    if let Some((slide_id, object_id)) = target {
                        controller.change_text(slide_id, object_id, new_text);
                        refresh();
                    }
                }
            }
            if let Some(canvas) = canvas.upgrade() {
                canvas.queue_draw();
                if restore_focus.replace(false) {
                    canvas.grab_focus();
                }
            }
        });
    }
    {
        let session = session.clone();
        let popover = popover.downgrade();
        let scrolled = scrolled.clone();
        h.canvas.connect_resize(move |canvas, _, _| {
            if let (Some(edit), Some(popover)) = (session.borrow().as_ref(), popover.upgrade()) {
                position_text_editor(canvas, &popover, &scrolled, edit.bounds);
            }
        });
    }
    {
        let slides = h.slides.clone();
        let controller = h.controller.clone();
        let refresh = h.refresh_hud.clone();
        let canvas = h.canvas.downgrade();
        let editing = editing.clone();
        let dbl = gtk::GestureClick::new();
        dbl.set_button(1);
        dbl.connect_pressed(move |gesture, n, x, y| {
            if n != 2 || editing.get() {
                return;
            }
            let Some(canvas) = canvas.upgrade() else { return };
            let (hx, hy) = canvas_to_slide(x, y, canvas.width() as f64, canvas.height() as f64);
            // Resolve the view index through the controller first: the
            // window's slide borrow below must never overlap a
            // controller call on the same RefCell.
            let Some(idx) = controller.selected_slide_index() else { return };
            let mut slides = slides.borrow_mut();
            decks_core::ops::ensure_ids(&mut slides);
            let Some(slide) = slides.get(idx) else { return };
            let Some(oi) = hit_test_object(&slide.objects, hx, hy) else { return };
            let SlideObject::TextBox { text, x, y, w, h, .. } = &slide.objects[oi] else { return };
            let edit = TextEdit {
                slide_id: slide.ids.slide,
                object_id: slide.ids.objects[oi],
                master: controller.editing_master(),
                original: text.clone(),
                bounds: (*x, *y, *w, *h),
            };
            let slide_id = edit.slide_id;
            let object_id = edit.object_id;
            drop(slides);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            controller.select_objects(slide_id, &[object_id], Some(object_id), decks_core::controller::SelectionMode::Object);
            refresh();
            canvas.queue_draw();
            let buffer = text_view.buffer();
            buffer.set_enable_undo(false);
            buffer.set_text(&edit.original);
            buffer.set_enable_undo(true);
            position_text_editor(&canvas, &popover, &scrolled, edit.bounds);
            *session.borrow_mut() = Some(edit);
            editing.set(true);
            popover.popup();
            text_view.grab_focus();
        });
        h.canvas.add_controller(dbl);
    }
    editing
}

fn position_text_editor(
    canvas: &gtk::DrawingArea,
    popover: &gtk::Popover,
    scrolled: &gtk::ScrolledWindow,
    (x, y, w, h): (f64, f64, f64, f64),
) {
    let width = canvas.width() as f64;
    let height = canvas.height() as f64;
    let (left, top) = crate::canvas::slide_to_canvas(x, y, width, height);
    let (right, bottom) = crate::canvas::slide_to_canvas(x + w, y + h, width, height);
    // The host is the surrounding overlay, not the canvas. Translate
    // through the scrolled viewport so zoom/scroll offsets are respected.
    let Some(host) = popover.parent() else { return };
    let Some(origin) = canvas.compute_point(&host, &gtk::graphene::Point::new(0.0, 0.0)) else { return };
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        (left + f64::from(origin.x())).round() as i32,
        (top + f64::from(origin.y())).round() as i32,
        (right - left).round().max(1.0) as i32,
        (bottom - top).round().max(1.0) as i32,
    )));
    // Tiny boxes still need usable controls; large boxes scroll rather than
    // forcing the native popup past the viewport.
    scrolled.set_size_request(
        (right - left).clamp(240.0, (width - 32.0).max(240.0)) as i32,
        (bottom - top).clamp(120.0, (height * 0.6).max(120.0)) as i32,
    );
}

/// The object Tab (or, `back`, Shift+Tab) selects next among `count`,
/// from `current`: in order, wrapping round, starting at the first (or the
/// last) when nothing is selected.
fn next_object(current: Option<usize>, count: usize, back: bool) -> usize {
    match (current, back) {
        (None, false) => 0,
        (None, true) => count - 1,
        (Some(i), false) => (i + 1) % count,
        (Some(i), true) => (i + count - 1) % count,
    }
}

#[cfg(test)]
mod tests {
    use super::next_object;

    #[test]
    fn tab_walks_the_objects_round_in_both_directions() {
        assert_eq!(next_object(None, 3, false), 0);
        assert_eq!(next_object(Some(2), 3, false), 0, "Tab wraps to the first");
        assert_eq!(next_object(None, 3, true), 2);
        assert_eq!(next_object(Some(0), 3, true), 2, "Shift+Tab wraps to the last");
        assert_eq!(next_object(Some(1), 3, true), 0);
    }
}

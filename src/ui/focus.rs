//! Keyboard focus checks that also hold while the window is in the background.
//!
//! While its window is inactive, GTK clears the focus state flags
//! (`FOCUS_WITHIN`) and `has_focus()` of the window's focus widget, yet the
//! window keeps it as its focus widget and gives it focus back when it is
//! activated. A refresh can happen then (auto-refresh), so code keeping focus
//! in place across it asks the window instead.

use gtk::prelude::*;

/// The focus widget of `widget`'s window, active or not.
fn window_focus(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Widget> {
    widget.root()?.focus()
}

/// Whether `widget` is its window's focus widget.
pub fn is_focus(widget: &impl IsA<gtk::Widget>) -> bool {
    window_focus(widget).is_some_and(|focus| &focus == widget.upcast_ref())
}

/// Whether `widget` or one of its descendants is its window's focus widget.
pub fn contains_focus(widget: &impl IsA<gtk::Widget>) -> bool {
    window_focus(widget).is_some_and(|focus| focus.is_ancestor(widget) || is_focus(widget))
}

/// The child of `widget` holding its window's focus widget, if any. Unlike
/// `focus_child()`, which GTK unsets while the window is inactive.
pub fn focused_child(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Widget> {
    let mut focus = window_focus(widget)?;
    loop {
        let parent = focus.parent()?;
        if &parent == widget.upcast_ref() {
            return Some(focus);
        }
        focus = parent;
    }
}

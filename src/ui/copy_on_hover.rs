use gtk::{glib, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

pub fn copy_text_to_clipboard(text: &str) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    display.clipboard().set_text(text);
}

fn build_copy_button(tooltip: &str, css_class: &str) -> (gtk::Button, gtk::Stack) {
    let copy_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_icon.set_pixel_size(14);

    let success_icon = gtk::Image::from_icon_name("object-select-symbolic");
    success_icon.set_pixel_size(14);

    let icon_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();
    icon_stack.add_named(&copy_icon, Some("copy"));
    icon_stack.add_named(&success_icon, Some("success"));
    icon_stack.set_visible_child_name("copy");

    let copy_button = gtk::Button::builder()
        .child(&icon_stack)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .build();
    copy_button.add_css_class("flat");
    copy_button.add_css_class(css_class);
    copy_button.set_can_focus(false);
    copy_button.set_width_request(22);
    copy_button.set_height_request(22);

    (copy_button, icon_stack)
}

fn wire_copy_button(
    copy_button: &gtk::Button,
    icon_stack: &gtk::Stack,
    copy_text: Rc<RefCell<String>>,
) {
    let icon_stack_for_click = icon_stack.clone();
    let copy_button_for_click = copy_button.clone();
    copy_button.connect_clicked(move |_| {
        copy_text_to_clipboard(&copy_text.borrow());

        icon_stack_for_click.set_visible_child_name("success");
        copy_button_for_click.add_css_class("success");

        let stack_clone = icon_stack_for_click.clone();
        let button_clone = copy_button_for_click.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            button_clone.remove_css_class("success");
            button_clone.remove_css_class("visible");

            let stack_inner = stack_clone.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                stack_inner.set_visible_child_name("copy");
            });
        });
    });
}

fn attach_hover_reveal(copy_button: &gtk::Button, hover_target: &impl IsA<gtk::Widget>) {
    let motion_controller = gtk::EventControllerMotion::new();
    let copy_button_for_enter = copy_button.clone();
    motion_controller.connect_enter(move |_, _, _| {
        copy_button_for_enter.add_css_class("visible");
    });
    let copy_button_for_leave = copy_button.clone();
    motion_controller.connect_leave(move |_| {
        if !copy_button_for_leave.has_css_class("success") {
            copy_button_for_leave.remove_css_class("visible");
        }
    });
    hover_target.add_controller(motion_controller);
}

pub struct CopyOnHoverRow {
    pub widget: gtk::Box,
    pub label: gtk::Label,
    copy_button: gtk::Button,
    pub copy_text: Rc<RefCell<String>>,
}

impl CopyOnHoverRow {
    pub fn new(
        label_text: &str,
        copy_text: String,
        tooltip: &str,
        css_class: &str,
        ellipsize: Option<gtk::pango::EllipsizeMode>,
    ) -> Self {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        widget.set_halign(gtk::Align::Start);

        let label = gtk::Label::builder().label(label_text).xalign(0.0).build();
        if let Some(mode) = ellipsize {
            label.set_ellipsize(mode);
        }

        let text_cell = Rc::new(RefCell::new(copy_text));
        let (copy_button, icon_stack) = build_copy_button(tooltip, css_class);
        wire_copy_button(&copy_button, &icon_stack, text_cell.clone());

        widget.append(&label);
        widget.append(&copy_button);

        Self {
            widget,
            label,
            copy_button,
            copy_text: text_cell,
        }
    }

    pub fn reveal_on_hover(&self, hover_target: &impl IsA<gtk::Widget>) {
        attach_hover_reveal(&self.copy_button, hover_target);
    }

    pub fn with_trailing_spacer(self) -> gtk::Box {
        let spacer = gtk::Box::builder().hexpand(true).build();
        self.widget.append(&spacer);
        self.widget
    }
}

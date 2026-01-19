mod imp;
use gtk::{glib, prelude::*, subclass::prelude::*};

pub struct Entry {
    pub name: String,
    pub tags: Vec<String>,
}

glib::wrapper! {
    pub struct GridCell(ObjectSubclass<imp::GridCell>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GridCell {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GridCell {
    pub fn set_entry(&self, entry: &Entry) {
        let imp = self.imp();
        let container = &imp.container;

        // Remove all children (chips will be re-added, name inscription stays via template)
        // We need to be careful - the name inscription is part of the template, so we
        // should remove it temporarily, clear chips, then re-add it
        let name_widget = imp.name.clone();
        container.remove(&name_widget);

        // Remove all remaining children (these are the chips)
        while let Some(child) = container.first_child() {
            container.remove(&child);
        }

        // Add chips for tags (chips should not expand)
        for tag_name in &entry.tags {
            let chip = gtk::Button::builder().label(tag_name).build();
            chip.add_css_class("tag-chip");
            chip.set_can_focus(false);
            chip.set_hexpand(false);
            container.append(&chip);
        }

        // Re-add the name inscription at the end (should expand to fill space)
        name_widget.set_hexpand(true);
        container.append(&name_widget);

        // Set the text
        imp.name.set_text(Some(&entry.name));
    }
}

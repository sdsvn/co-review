//! Pickers and prompts in Lapce's palette (`PaletteKind::Review`): the review switcher, Go to Comment…, the ⋯ menu,
//! Connect agent…, Choose Agent Model…, and text prompts (a review's title, a custom agent command).
//!
//! The palette filters its items on a background thread, so an item only carries an index into the picker that is
//! open; the picker itself (with its actions) stays on the UI thread here.

use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use floem::{
    reactive::{ReadSignal, SignalGet, SignalUpdate},
    style::AlignItems,
    views::{Container, Decorators, container, label, stack},
};
use lapce_xi_rope::Rope;

use crate::{
    config::{LapceConfig, color::LapceColor},
    focus_text::focus_text,
    palette::{
        PaletteData,
        item::{PaletteItem, PaletteItemContent},
        kind::PaletteKind,
    },
};

/// What a picker does with a choice: called with the text typed in the palette.
pub type PickerAction = Box<dyn FnOnce(String)>;

/// One entry of a picker, as the palette lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerItem {
    /// The picker this entry belongs to (a newer picker replaces an older one).
    pub picker: u64,
    pub index: usize,
    pub description: Option<String>,
    /// Listed whatever is typed: the entry that takes the typed text, of a prompt.
    pub always_shown: bool,
}

/// A choice in a picker.
pub struct Choice {
    pub label: String,
    pub description: Option<String>,
    pub action: PickerAction,
}

impl Choice {
    pub fn new(label: impl Into<String>, action: impl FnOnce() + 'static) -> Self {
        Self {
            label: label.into(),
            description: None,
            action: Box::new(move |_| action()),
        }
    }

    pub fn described(mut self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.description = (!description.is_empty()).then_some(description);
        self
    }
}

/// A list to pick from, or a prompt for text.
pub struct Picker {
    /// Shown in the empty palette input.
    pub placeholder: &'static str,
    pub choices: Vec<Choice>,
    /// A prompt: the text is typed, and this entry (always listed) takes it.
    pub prompt: Option<(String, PickerAction)>,
    /// The text the palette starts with (a prompt's current value).
    pub value: String,
}

impl Picker {
    pub fn choose(placeholder: &'static str, choices: Vec<Choice>) -> Self {
        Self {
            placeholder,
            choices,
            prompt: None,
            value: String::new(),
        }
    }

    /// Asks for a line of text, starting with `value`; Enter calls `action` with it (Escape cancels).
    pub fn prompt(
        placeholder: &'static str,
        value: impl Into<String>,
        hint: impl Into<String>,
        action: impl FnOnce(String) + 'static,
    ) -> Self {
        Self {
            placeholder,
            choices: Vec::new(),
            prompt: Some((hint.into(), Box::new(action))),
            value: value.into(),
        }
    }
}

struct OpenPicker {
    id: u64,
    placeholder: &'static str,
    items: im::Vector<PaletteItem>,
    actions: Vec<Option<PickerAction>>,
}

thread_local! {
    static OPEN: RefCell<Option<OpenPicker>> = const { RefCell::new(None) };
    static NEXT_ID: Cell<u64> = const { Cell::new(0) };
}

/// Shows `picker` in the window tab's palette.
pub fn open(palette: &PaletteData, picker: Picker) {
    let id = NEXT_ID.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let entries = picker
        .prompt
        .map(|(hint, action)| (hint, None, true, action))
        .into_iter()
        .chain(picker.choices.into_iter().map(|choice| (choice.label, choice.description, false, choice.action)));
    let mut items = im::Vector::new();
    let mut actions = Vec::new();
    for (index, (filter_text, description, always_shown, action)) in entries.enumerate() {
        items.push_back(PaletteItem {
            content: PaletteItemContent::Review(PickerItem {
                picker: id,
                index,
                description,
                always_shown,
            }),
            filter_text,
            score: 0,
            indices: Vec::new(),
        });
        actions.push(Some(action));
    }
    OPEN.with(|open| {
        *open.borrow_mut() = Some(OpenPicker {
            id,
            placeholder: picker.placeholder,
            items,
            actions,
        })
    });
    palette.run(PaletteKind::Review);
    if !picker.value.is_empty() {
        let length = picker.value.len();
        palette.input_editor.doc().reload(Rope::from(picker.value), true);
        palette.input_editor.cursor().update(|cursor| {
            cursor.set_insert(lapce_core::selection::Selection::caret(length))
        });
    }
}

/// The open picker's items, for the palette (`PaletteKind::Review`).
pub fn palette_items() -> im::Vector<PaletteItem> {
    OPEN.with(|open| open.borrow().as_ref().map(|open| open.items.clone()).unwrap_or_default())
}

/// The open picker's placeholder.
pub fn palette_placeholder() -> &'static str {
    OPEN.with(|open| open.borrow().as_ref().map_or("", |open| open.placeholder))
}

/// The palette picked `item`, with `input` typed.
pub fn palette_selected(item: &PickerItem, input: String) {
    let action = OPEN.with(|open| {
        let mut open = open.borrow_mut();
        let picker = open.as_mut().filter(|picker| picker.id == item.picker)?;
        picker.actions.get_mut(item.index)?.take()
    });
    if let Some(action) = action {
        action(input);
    }
}

/// How the palette lists a picker entry: its label (matched characters highlighted) and its description, dimmed.
pub fn palette_item_view(
    item: &PaletteItem,
    picker_item: &PickerItem,
    config: ReadSignal<Arc<LapceConfig>>,
) -> Container {
    let text = item.filter_text.clone();
    let indices = item.indices.clone();
    let description = picker_item.description.clone().unwrap_or_default();
    let has_description = !description.is_empty();
    container(
        stack((
            focus_text(
                move || text.clone(),
                move || indices.clone(),
                move || config.get().color(LapceColor::EDITOR_FOCUS),
            )
            .style(|s| s.margin_right(8.0).max_width_full()),
            label(move || description.clone()).style(move |s| {
                s.color(config.get().color(LapceColor::EDITOR_DIM))
                    .min_width(0.0)
                    .flex_grow(1.0f32)
                    .flex_basis(0.0)
                    .apply_if(!has_description, |s| s.hide())
            }),
        ))
        .style(|s| s.align_items(Some(AlignItems::Center)).max_width_full()),
    )
}

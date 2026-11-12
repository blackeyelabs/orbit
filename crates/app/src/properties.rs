//! The properties panel: a note's properties, each with an editor, under its
//! inline title in Live Preview. Each key gets an editor for its type,
//! guessed from its value; `tags` and `aliases` are chips, tags suggested
//! from the vault's. An edit is handed to the note as a
//! [`PropertyEdit`], which changes only that key's lines (ADR 0009), so
//! undo takes it back like any other. YAML the panel can't edit safely
//! offers "Edit in Source", and a value that doesn't fit its type is
//! flagged, never rewritten. The file's modified time shows read-only.

use std::{sync::Arc, time::SystemTime};

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use md_engine::{Property, PropertyEdit, PropertyType, Value, front_matter, properties};
use orbit_core::{PropertyTypes, Vault, VaultPath, same_tag};

actions!(
    properties,
    [
        /// Up in a tag field: the suggestion above.
        SuggestionUp,
        /// Down in a tag field: the suggestion below.
        SuggestionDown
    ]
);

/// A chip field's context, for its suggestion keys.
const CHIPS_CONTEXT: &str = "PropertyChips";
/// How many tags are suggested at once.
const MAX_SUGGESTIONS: usize = 6;
/// How wide a key's name is drawn.
const KEY_WIDTH: Pixels = px(150.);

pub fn init(cx: &mut App) {
    let context = format!("{CHIPS_CONTEXT} > Input");
    let context = Some(context.as_str());
    cx.bind_keys([
        KeyBinding::new("up", SuggestionUp, context),
        KeyBinding::new("down", SuggestionDown, context),
    ]);
}

pub enum PropertiesEvent {
    /// The person changed a property: the edit to make to the note.
    Edit(PropertyEdit),
    /// "Edit in Source" for the key whose lines start at this offset.
    EditInSource(usize),
    /// A type was chosen for the key, by hand.
    SetType { key: String, kind: PropertyType },
}

impl EventEmitter<PropertiesEvent> for PropertiesPanel {}

pub struct PropertiesPanel {
    vault: Arc<Vault>,
    /// The note the properties are of, whose types apply.
    note: VaultPath,
    /// The vault's property types as of the rows shown.
    types_seen: u64,
    rows: Vec<Row>,
    /// The front matter the rows show, to see when it changed.
    shown: Option<String>,
    /// The name field "Add property" opened, while it's open.
    adding: Option<(Entity<InputState>, [Subscription; 2])>,
    /// A key just added, whose value field takes focus once it's shown.
    focus_value_of: Option<String>,
    /// The file's modified time, formatted.
    modified: Option<SharedString>,
}

/// One key, with the fields that edit it.
struct Row {
    property: Property,
    /// Its type; `None` for YAML only Source mode edits.
    kind: Option<PropertyType>,
    /// How its value is edited: by its type, or as the value is written
    /// when that doesn't fit the type.
    shape: Shape,
    name: Entity<InputState>,
    editor: Editor,
    /// What its value field holds doesn't fit its type.
    warning: bool,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Field(PropertyType),
    Checkbox,
    Chips,
    Source,
}

enum Editor {
    Field(Entity<InputState>),
    Checkbox,
    Chips(Chips),
    Source,
}

/// A list's items as chips, and the field that adds one.
struct Chips {
    input: Entity<InputState>,
    /// Tags from the vault that match what's typed.
    suggestions: Vec<String>,
    highlighted: Option<usize>,
}

impl PropertiesPanel {
    pub fn new(vault: Arc<Vault>, note: VaultPath) -> Self {
        PropertiesPanel {
            types_seen: vault.property_types_revision(),
            vault,
            note,
            rows: Vec::new(),
            shown: None,
            adding: None,
            focus_value_of: None,
            modified: None,
        }
    }

    /// Shows the front matter of `source`, if it changed since last
    /// shown, or a property type was set since (here or in another note).
    pub fn sync(&mut self, source: &str, window: &mut Window, cx: &mut Context<Self>) {
        let front = front_matter(source);
        let types_same = self.types_seen == self.vault.property_types_revision();
        if self.shown.as_deref() == Some(front) && types_same {
            return;
        }
        self.shown = Some(front.to_string());
        self.types_seen = self.vault.property_types_revision();
        let types = self.vault.property_types(&self.note);
        let read = properties(source);
        self.rows.truncate(read.len());
        for (ix, property) in read.into_iter().enumerate() {
            let kind = kind_of(&property, &types);
            let shape = shape_of(&property, kind);
            match self.rows.get_mut(ix) {
                Some(row) if row.shape == shape => row.show(property, kind, window, cx),
                _ => {
                    let row = self.new_row(ix, property, kind, shape, window, cx);
                    if ix < self.rows.len() {
                        self.rows[ix] = row;
                    } else {
                        self.rows.push(row);
                    }
                }
            }
        }
        if let Some(key) = self.focus_value_of.take()
            && let Some(row) = self.rows.iter().find(|r| r.property.key == key)
        {
            row.focus_value(window, cx);
        }
        cx.notify();
    }

    /// The note the properties are of: it moved.
    pub fn set_note(&mut self, note: VaultPath) {
        self.note = note;
        self.shown = None;
    }

    /// Shows the front matter again on the next [`Self::sync`], all of
    /// it, as an edit was refused.
    pub fn forget_shown(&mut self) {
        self.shown = None;
    }

    /// Shows `modified` as the file's modified time.
    pub fn set_modified(&mut self, modified: Option<SystemTime>, cx: &mut Context<Self>) {
        self.modified = modified.map(|time| {
            let local: chrono::DateTime<chrono::Local> = time.into();
            local.format("%Y-%m-%d %H:%M").to_string().into()
        });
        cx.notify();
    }

    fn new_row(
        &mut self,
        ix: usize,
        property: Property,
        kind: Option<PropertyType>,
        shape: Shape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Row {
        let name = cx.new(|cx| InputState::new(window, cx).default_value(property.key.clone()));
        let mut subscriptions =
            vec![
                cx.subscribe_in(&name, window, move |this, _, event, _, cx| {
                    if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                        this.rename(ix, cx);
                    }
                }),
            ];
        let editor = match shape {
            Shape::Field(kind) => {
                let field = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder(kind))
                        .default_value(field_text(&property.value))
                });
                subscriptions.push(cx.subscribe_in(
                    &field,
                    window,
                    move |this, _, event, _, cx| match event {
                        InputEvent::Change => this.field_changed(ix, cx),
                        InputEvent::PressEnter { .. } | InputEvent::Blur => {
                            this.commit_field(ix, cx)
                        }
                        InputEvent::Focus => {}
                    },
                ));
                Editor::Field(field)
            }
            Shape::Checkbox => Editor::Checkbox,
            Shape::Chips => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("Add…"));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event, window, cx| match event {
                        InputEvent::Change => this.suggest(ix, cx),
                        InputEvent::PressEnter { .. } => this.add_chip(ix, None, window, cx),
                        InputEvent::Blur | InputEvent::Focus => {}
                    },
                ));
                Editor::Chips(Chips {
                    input,
                    suggestions: Vec::new(),
                    highlighted: None,
                })
            }
            Shape::Source => Editor::Source,
        };
        let warning = kind.is_some_and(|k| !k.fits(&property.value));
        Row {
            property,
            kind,
            shape,
            name,
            editor,
            warning,
            _subscriptions: subscriptions,
        }
    }

    /// The name field was left or Enter pressed: renames the key if it
    /// was changed.
    fn rename(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let to = row.name.read(cx).value().trim().to_string();
        if to != row.property.key {
            cx.emit(PropertiesEvent::Edit(PropertyEdit::Rename {
                from: row.property.key.clone(),
                to,
            }));
        }
    }

    /// Typing in a value field: flags what doesn't fit, as it's typed.
    fn field_changed(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get_mut(ix) else {
            return;
        };
        if let (Editor::Field(field), Shape::Field(shape)) = (&row.editor, row.shape) {
            let typed = typed(row.kind, shape, &field.read(cx).value());
            row.warning = row.kind.is_some_and(|k| !k.fits(&typed));
            cx.notify();
        }
    }

    /// A value field was left or Enter pressed: sets the value if it was
    /// changed.
    /// (Text left as shown isn't written again, even where the field
    /// can't show it exactly, such as quoted spaces.)
    fn commit_field(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        if let (Editor::Field(field), Shape::Field(shape)) = (&row.editor, row.shape) {
            let text = field.read(cx).value();
            if text != field_text(&row.property.value) {
                let value = typed(row.kind, shape, &text);
                self.set(ix, value, cx);
            }
        }
    }

    fn set(&self, ix: usize, value: Value, cx: &mut Context<Self>) {
        let key = self.rows[ix].property.key.clone();
        cx.emit(PropertiesEvent::Edit(PropertyEdit::Set { key, value }));
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.rows.get(ix).map(|r| &r.property.value) {
            Some(Value::Checkbox(on)) => self.set(ix, Value::Checkbox(!on), cx),
            // An empty checkbox is unticked.
            Some(Value::Text(s)) if s.is_empty() => self.set(ix, Value::Checkbox(true), cx),
            _ => {}
        }
    }

    fn set_type(&mut self, ix: usize, kind: PropertyType, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            let key = row.property.key.clone();
            cx.emit(PropertiesEvent::SetType { key, kind });
        }
    }

    fn remove(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            let key = row.property.key.clone();
            cx.emit(PropertiesEvent::Edit(PropertyEdit::Remove { key }));
        }
    }

    fn edit_in_source(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            cx.emit(PropertiesEvent::EditInSource(row.property.lines.start));
        }
    }

    /// A list's items, from a list or a lone value.
    fn items(&self, ix: usize) -> Vec<String> {
        match &self.rows[ix].property.value {
            Value::List(items) => items.clone(),
            Value::Text(s) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    /// Adds a chip: `picked`, else the suggestion highlighted, else what's
    /// typed. A tag goes without its `#`, and not twice.
    fn add_chip(
        &mut self,
        ix: usize,
        picked: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Editor::Chips(chips)) = self.rows.get(ix).map(|r| &r.editor) else {
            return;
        };
        let typed = chips.input.read(cx).value().trim().to_string();
        let highlighted = chips.highlighted.map(|h| chips.suggestions[h].clone());
        let input = chips.input.clone();
        let item = picked.or(highlighted).unwrap_or(typed);
        let is_tags = self.rows[ix].kind == Some(PropertyType::Tags);
        let item = if is_tags {
            item.trim_start_matches('#').to_string()
        } else {
            item
        };
        input.update(cx, |input, cx| input.set_value("", window, cx));
        if let Some(Editor::Chips(chips)) = self.rows.get_mut(ix).map(|r| &mut r.editor) {
            chips.suggestions.clear();
            chips.highlighted = None;
        }
        let mut items = self.items(ix);
        let same = |a: &str, b: &str| if is_tags { same_tag(a, b) } else { a == b };
        if item.is_empty() || items.iter().any(|i| same(i, &item)) {
            return cx.notify();
        }
        items.push(item);
        self.set(ix, Value::List(items), cx);
    }

    fn remove_chip(&mut self, ix: usize, item: usize, cx: &mut Context<Self>) {
        let mut items = self.items(ix);
        if item < items.len() {
            items.remove(item);
            self.set(ix, Value::List(items), cx);
        }
    }

    /// The vault's tags that match what's typed in a tag field: those
    /// starting with it first, then those holding it.
    fn suggest(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let Editor::Chips(chips) = &row.editor else {
            return;
        };
        if row.kind != Some(PropertyType::Tags) {
            return;
        }
        let typed = chips.input.read(cx).value();
        let typed = typed.trim().trim_start_matches('#').to_lowercase();
        let have = self.items(ix);
        // Those starting with it first, then by name ignoring case.
        let mut found: Vec<(bool, String, String)> = if typed.is_empty() {
            Vec::new()
        } else {
            self.vault
                .tags()
                .into_iter()
                .map(|t| t.name)
                .filter(|name| !have.iter().any(|h| same_tag(h, name)))
                .filter_map(|name| {
                    let lower = name.to_lowercase();
                    let starts = lower.starts_with(&typed);
                    lower.contains(&typed).then_some((!starts, lower, name))
                })
                .collect()
        };
        found.sort();
        if let Some(Editor::Chips(chips)) = self.rows.get_mut(ix).map(|r| &mut r.editor) {
            chips.suggestions = found
                .into_iter()
                .take(MAX_SUGGESTIONS)
                .map(|(_, _, name)| name)
                .collect();
            chips.highlighted = None;
        }
        cx.notify();
    }

    /// Up or Down in a tag field: moves the highlight among the
    /// suggestions.
    fn move_highlight(&mut self, ix: usize, down: bool, cx: &mut Context<Self>) {
        if let Some(Editor::Chips(chips)) = self.rows.get_mut(ix).map(|r| &mut r.editor) {
            let len = chips.suggestions.len();
            if len == 0 {
                return;
            }
            chips.highlighted = Some(match (chips.highlighted, down) {
                (None, true) => 0,
                (None, false) => len - 1,
                (Some(h), true) => (h + 1) % len,
                (Some(h), false) => (h + len - 1) % len,
            });
            cx.notify();
        }
    }

    /// "Add property": a field for the new key's name.
    fn start_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Property name"));
        let enter = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.finish_adding(window, cx);
            }
        });
        let blur = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if let InputEvent::Blur = event {
                this.adding = None;
                cx.notify();
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.adding = Some((input, [enter, blur]));
        cx.notify();
    }

    /// Enter in the new key's name: adds it, empty, and its value field
    /// takes focus.
    fn finish_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((input, _)) = self.adding.take() else {
            return;
        };
        let key = input.read(cx).value().trim().to_string();
        cx.notify();
        if key.is_empty() {
            return;
        }
        // A key the note has already: its value is what's wanted.
        if let Some(row) = self.rows.iter().find(|r| r.property.key == key) {
            return row.focus_value(window, cx);
        }
        self.focus_value_of = Some(key.clone());
        cx.emit(PropertiesEvent::Edit(PropertyEdit::Set {
            key,
            value: Value::Text(String::new()),
        }));
    }
}

impl Row {
    /// Shows `property`, read again, in the fields it already has.
    fn show(
        &mut self,
        property: Property,
        kind: Option<PropertyType>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let set = |input: &Entity<InputState>, text: String, window: &mut Window, cx: &mut App| {
            if input.read(cx).value() != text {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        };
        set(&self.name, property.key.clone(), window, cx);
        if let Editor::Field(field) = &self.editor {
            set(field, field_text(&property.value), window, cx);
        }
        self.kind = kind;
        self.warning = self.kind.is_some_and(|k| !k.fits(&property.value));
        self.property = property;
    }

    fn focus_value(&self, window: &mut Window, cx: &mut App) {
        match &self.editor {
            Editor::Field(input) => input.update(cx, |input, cx| input.focus(window, cx)),
            Editor::Chips(chips) => chips.input.update(cx, |input, cx| input.focus(window, cx)),
            Editor::Checkbox | Editor::Source => {}
        }
    }
}

/// How a property's value is edited: by its type when the value fits it,
/// else as the value is written, so nothing is rewritten unasked.
fn shape_of(property: &Property, kind: Option<PropertyType>) -> Shape {
    let Some(kind) = kind else {
        return Shape::Source;
    };
    let kind = if kind.fits(&property.value) {
        kind
    } else {
        match PropertyType::of_value(&property.value) {
            Some(own) => own,
            None => return Shape::Source,
        }
    };
    match kind {
        PropertyType::Checkbox => Shape::Checkbox,
        PropertyType::List | PropertyType::Tags | PropertyType::Aliases => Shape::Chips,
        kind => Shape::Field(kind),
    }
}

/// A property's type: one set by hand, else guessed from its value.
/// `tags` and `aliases` are always theirs.
fn kind_of(property: &Property, types: &PropertyTypes) -> Option<PropertyType> {
    let guessed = property.guessed_type()?;
    if guessed.is_fixed() {
        return Some(guessed);
    }
    types.get(&property.key).or(Some(guessed))
}

/// A value field's text for `value`.
fn field_text(value: &Value) -> String {
    match value {
        Value::Text(s) | Value::Number(s) | Value::Date(s) | Value::DateTime(s) => s.clone(),
        Value::Checkbox(b) => b.to_string(),
        Value::List(items) => items.join(", "),
        Value::Unsupported => String::new(),
    }
}

/// What's typed in a field as a value: of the row's type `kind` if it
/// fits, else of the field's own `shape` if it fits that, else text.
fn typed(kind: Option<PropertyType>, shape: PropertyType, text: &str) -> Value {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Value::Text(text);
    }
    let as_kind = |kind: PropertyType| match kind {
        PropertyType::Number => Value::Number(text.clone()),
        PropertyType::Date => Value::Date(text.clone()),
        PropertyType::DateTime => Value::DateTime(text.clone()),
        _ => Value::Text(text.clone()),
    };
    kind.into_iter()
        .chain([shape])
        .map(|k| (k, as_kind(k)))
        .find(|(k, value)| k.fits(value))
        .map_or_else(|| Value::Text(text.clone()), |(_, value)| value)
}

fn type_label(kind: PropertyType) -> &'static str {
    match kind {
        PropertyType::Text => "Text",
        PropertyType::List => "List",
        PropertyType::Number => "Number",
        PropertyType::Checkbox => "Checkbox",
        PropertyType::Date => "Date",
        PropertyType::DateTime => "Date & time",
        PropertyType::Tags => "Tags",
        PropertyType::Aliases => "Aliases",
    }
}

fn placeholder(kind: PropertyType) -> &'static str {
    match kind {
        PropertyType::Number => "Number",
        PropertyType::Date => "YYYY-MM-DD",
        PropertyType::DateTime => "YYYY-MM-DDTHH:mm",
        _ => "Empty",
    }
}

fn icon(kind: Option<PropertyType>) -> IconName {
    match kind {
        Some(PropertyType::Text) => IconName::Type,
        Some(PropertyType::List) => IconName::List,
        Some(PropertyType::Number) => IconName::Binary,
        Some(PropertyType::Checkbox) => IconName::SquareCheck,
        Some(PropertyType::Date) => IconName::Calendar,
        Some(PropertyType::DateTime) => IconName::CalendarClock,
        Some(PropertyType::Tags) => IconName::Tags,
        Some(PropertyType::Aliases) => IconName::Link,
        None => IconName::Info,
    }
}

impl Render for PropertiesPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = (0..self.rows.len())
            .map(|ix| self.render_row(ix, cx))
            .collect();
        let adding = match &self.adding {
            Some((input, _)) => div()
                .w(KEY_WIDTH)
                .child(Input::new(input).small())
                .into_any_element(),
            None => Button::new("add-property")
                .ghost()
                .small()
                .icon(IconName::Plus)
                .label("Add property")
                .on_click(cx.listener(|this, _, window, cx| this.start_adding(window, cx)))
                .into_any_element(),
        };
        let modified = self.modified.clone().map(|time| {
            h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Icon::new(IconName::Clock).small())
                .child(div().w(KEY_WIDTH).child("Modified"))
                .child(time)
        });
        v_flex()
            .id("properties")
            .flex_none()
            .px_4()
            .pb_2()
            .gap_1()
            .max_h(px(280.))
            .overflow_y_scroll()
            .children(rows)
            .child(adding)
            .children(modified)
    }
}

impl PropertiesPanel {
    fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[ix];
        let theme = cx.theme();
        let (muted, warning) = (theme.muted_foreground, theme.warning);
        let name = div()
            .w(KEY_WIDTH)
            .flex_none()
            .child(Input::new(&row.name).small().appearance(false));
        let value: AnyElement = match &row.editor {
            Editor::Field(field) => Input::new(field)
                .small()
                .appearance(false)
                .into_any_element(),
            Editor::Checkbox => Checkbox::new(("property-check", ix))
                .checked(row.property.value == Value::Checkbox(true))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx)))
                .into_any_element(),
            Editor::Chips(chips) => self.render_chips(ix, chips, cx),
            Editor::Source => h_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Can't be edited here"),
                )
                .child(
                    Button::new(("edit-in-source", ix))
                        .outline()
                        .xsmall()
                        .label("Edit in Source")
                        .on_click(cx.listener(move |this, _, _, cx| this.edit_in_source(ix, cx))),
                )
                .into_any_element(),
        };
        h_flex()
            .id(("property", ix))
            .group("property")
            .gap_2()
            .min_h(rems(1.75))
            .child(self.render_type(ix, cx))
            .child(name)
            .child(div().flex_1().min_w_0().child(value))
            .when(row.warning, |this| {
                this.child(
                    div()
                        .id(("property-warning", ix))
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .small()
                                .text_color(warning),
                        )
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(
                                "Doesn't fit its type; left as written",
                            )
                            .build(window, cx)
                        }),
                )
            })
            .when(row.shape != Shape::Source, |this| {
                this.child(
                    Button::new(("remove-property", ix))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Remove property")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove(ix, cx))),
                )
            })
            .into_any_element()
    }

    /// The row's type icon: for a key whose type can be set, a menu of
    /// the types, the current one ticked.
    fn render_type(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let kind = self.rows[ix].kind;
        if kind.is_none_or(PropertyType::is_fixed) {
            let muted = cx.theme().muted_foreground;
            return Icon::new(icon(kind))
                .small()
                .text_color(muted)
                .into_any_element();
        }
        let panel = cx.entity().downgrade();
        Button::new(("property-type", ix))
            .ghost()
            .xsmall()
            .icon(icon(kind))
            .tooltip("Property type")
            .dropdown_menu(move |mut menu, _, _| {
                // Every type but a key's own (`tags`, `aliases`).
                for choice in PropertyType::ALL.into_iter().filter(|t| !t.is_fixed()) {
                    let panel = panel.clone();
                    menu = menu.item(
                        PopupMenuItem::new(type_label(choice))
                            .icon(icon(Some(choice)))
                            .checked(kind == Some(choice))
                            .on_click(move |_, _, cx| {
                                panel
                                    .update(cx, |panel, cx| panel.set_type(ix, choice, cx))
                                    .ok();
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_chips(&self, ix: usize, chips: &Chips, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, accent, muted) = (theme.border, theme.accent, theme.muted_foreground);
        let is_tags = self.rows[ix].kind == Some(PropertyType::Tags);
        let chip_row = self.items(ix).into_iter().enumerate().map(|(item, text)| {
            h_flex()
                .id(("chip", ix * 1000 + item))
                .gap_1()
                .pl_2()
                .pr_1()
                .rounded_full()
                .border_1()
                .border_color(border)
                .text_xs()
                .child(if is_tags { format!("#{text}") } else { text })
                .child(
                    div()
                        .id(("remove-chip", ix * 1000 + item))
                        .cursor_pointer()
                        .text_color(muted)
                        .child(Icon::new(IconName::Close).xsmall())
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_chip(ix, item, cx)),
                        ),
                )
        });
        let suggestions = chips.suggestions.iter().enumerate().map(|(s, name)| {
            let picked = name.clone();
            div()
                .id(("suggestion", ix * 1000 + s))
                .px_2()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .when(chips.highlighted == Some(s), |this| this.bg(accent))
                .hover(|this| this.bg(accent))
                .child(format!("#{name}"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.add_chip(ix, Some(picked.clone()), window, cx)
                }))
        });
        v_flex()
            .child(
                h_flex().flex_wrap().gap_1().children(chip_row).child(
                    div()
                        .key_context(CHIPS_CONTEXT)
                        .min_w(px(80.))
                        .flex_1()
                        .on_action(cx.listener(move |this, _: &SuggestionUp, _, cx| {
                            this.move_highlight(ix, false, cx)
                        }))
                        .on_action(cx.listener(move |this, _: &SuggestionDown, _, cx| {
                            this.move_highlight(ix, true, cx)
                        }))
                        .child(Input::new(&chips.input).small().appearance(false)),
                ),
            )
            .when(!chips.suggestions.is_empty(), |this| {
                this.child(
                    v_flex()
                        .mt_1()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .children(suggestions),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
impl PropertiesPanel {
    /// Focuses `key`'s value field, or the field that adds to its list.
    pub(crate) fn focus_value(&self, key: &str, window: &mut Window, cx: &mut App) {
        let row = self.rows.iter().find(|r| r.property.key == key);
        row.expect("the key has a row").focus_value(window, cx);
    }

    /// Focuses `key`'s name field.
    pub(crate) fn focus_name(&self, key: &str, window: &mut Window, cx: &mut App) {
        let row = self.rows.iter().find(|r| r.property.key == key);
        let name = &row.expect("the key has a row").name;
        name.update(cx, |name, cx| name.focus(window, cx));
    }

    /// What each row's name field and value field show (the value empty
    /// for other editors), and whether it's flagged.
    pub(crate) fn shown_rows(&self, cx: &App) -> Vec<(String, String, bool)> {
        self.rows
            .iter()
            .map(|r| {
                let value = match &r.editor {
                    Editor::Field(field) => field.read(cx).value().to_string(),
                    Editor::Source => "Edit in Source".into(),
                    _ => String::new(),
                };
                (r.name.read(cx).value().to_string(), value, r.warning)
            })
            .collect()
    }

    pub(crate) fn suggestions(&self, key: &str) -> Vec<String> {
        match self
            .rows
            .iter()
            .find(|r| r.property.key == key)
            .map(|r| &r.editor)
        {
            Some(Editor::Chips(chips)) => chips.suggestions.clone(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn click_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_adding(window, cx);
    }

    pub(crate) fn click_remove(&mut self, key: &str, cx: &mut Context<Self>) {
        let ix = self.rows.iter().position(|r| r.property.key == key);
        self.remove(ix.expect("the key has a row"), cx);
    }

    pub(crate) fn click_edit_in_source(&mut self, key: &str, cx: &mut Context<Self>) {
        let ix = self.rows.iter().position(|r| r.property.key == key);
        self.edit_in_source(ix.expect("the key has a row"), cx);
    }

    pub(crate) fn click_checkbox(&mut self, key: &str, cx: &mut Context<Self>) {
        let ix = self.rows.iter().position(|r| r.property.key == key);
        self.toggle(ix.expect("the key has a row"), cx);
    }

    /// Chooses `kind` in `key`'s type menu.
    pub(crate) fn click_type(&mut self, key: &str, kind: PropertyType, cx: &mut Context<Self>) {
        let ix = self.rows.iter().position(|r| r.property.key == key);
        self.set_type(ix.expect("the key has a row"), kind, cx);
    }

    /// The type `key`'s row has.
    pub(crate) fn kind(&self, key: &str) -> Option<PropertyType> {
        self.rows.iter().find(|r| r.property.key == key)?.kind
    }

    pub(crate) fn modified(&self) -> Option<SharedString> {
        self.modified.clone()
    }
}

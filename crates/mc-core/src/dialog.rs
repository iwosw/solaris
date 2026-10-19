//! Server-driven 26.1.2 dialogs: build a dialog, show it to one connection, and
//! receive the click as an ordinary [`PlayerInput::DialogClick`](crate::boundary::PlayerInput).
//!
//! Authority split (ADR 0013): the native tick decides *what* to show and acts on
//! the click; the edge only turns [`PlayerChange::ShowDialog`]/[`PlayerChange::ClearDialog`]
//! into `show_dialog`/`clear_dialog` packets and `custom_click_action` into the input.
//!
//! Every button built here uses vanilla's `dynamic/custom` action, so the click
//! payload is a compound holding the button's `additions` plus one entry per dialog
//! input, typed exactly as the vanilla client sends them: text -> String,
//! single_option -> String (the option id), boolean -> Byte, number_range -> Float.
//! A client can send any id/payload at any time (vanilla servers treat custom
//! click actions as untrusted too), so owners MUST validate permission, range and
//! state again in the tick before acting.
//!
//! Action ids are `solaris:<owner>/<action>`; [`route_click`] dispatches on `<owner>`.
//! New owners add one arm there.
use crate::boundary::{ChangeSet, ConnectionId, PlayerChange};
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag};

/// Why a dialog could not be built; vanilla clients disconnect on a malformed dialog,
/// so these are rejected before anything reaches the wire.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid dialog: {0}")]
pub struct DialogError(pub String);

fn err<T>(message: impl Into<String>) -> Result<T, DialogError> {
    Err(DialogError(message.into()))
}

/// What the client does after a button is pressed (`DialogAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterAction {
    /// Close the dialog (vanilla default).
    Close,
    /// Keep the dialog open, inputs retained.
    None,
    /// Show the "waiting for response" screen until the server answers
    /// (with another dialog or [`clear`]).
    WaitForResponse,
}

impl AfterAction {
    const fn name(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::None => "none",
            Self::WaitForResponse => "wait_for_response",
        }
    }
}

/// A text component: a literal string, or any component compound (`{translate: ..}`).
#[derive(Debug, Clone, PartialEq)]
pub struct Text(pub Tag);

impl From<&str> for Text {
    fn from(value: &str) -> Self {
        Self(Tag::String(value.to_owned()))
    }
}

impl From<String> for Text {
    fn from(value: String) -> Self {
        Self(Tag::String(value))
    }
}

impl From<Tag> for Text {
    fn from(value: Tag) -> Self {
        Self(value)
    }
}

/// One dialog button (`ActionButton`). Without an action it only runs `after_action`.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    label: Tag,
    tooltip: Option<Tag>,
    width: u32,
    action: Option<(String, Vec<(String, Tag)>)>,
}

impl Button {
    /// Button that only closes/continues per the dialog's `after_action`.
    pub fn plain(label: impl Into<Text>) -> Self {
        Self {
            label: label.into().0,
            tooltip: None,
            width: 150,
            action: None,
        }
    }

    /// Button whose click sends `custom_click_action(id, {additions.., inputs..})`.
    pub fn custom(label: impl Into<Text>, id: &str) -> Self {
        Self {
            action: Some((id.to_owned(), Vec::new())),
            ..Self::plain(label)
        }
    }

    /// Constant extra payload entry; overwritten by an input with the same key.
    #[must_use]
    pub fn with(mut self, key: &str, value: Tag) -> Self {
        if let Some((_, additions)) = &mut self.action {
            additions.push((key.to_owned(), value));
        }
        self
    }

    #[must_use]
    pub fn tooltip(mut self, tooltip: impl Into<Text>) -> Self {
        self.tooltip = Some(tooltip.into().0);
        self
    }

    #[must_use]
    pub fn width(mut self, width: u32) -> Self {
        self.width = width;
        self
    }

    fn to_tag(&self) -> Result<Tag, DialogError> {
        check_width(self.width)?;
        let mut entries = vec![("label".to_owned(), self.label.clone())];
        if let Some(tooltip) = &self.tooltip {
            entries.push(("tooltip".to_owned(), tooltip.clone()));
        }
        entries.push(("width".to_owned(), Tag::Int(self.width as i32)));
        if let Some((id, additions)) = &self.action {
            check_identifier(id)?;
            let mut action = vec![
                ("type".to_owned(), string("minecraft:dynamic/custom")),
                ("id".to_owned(), string(id)),
            ];
            if !additions.is_empty() {
                action.push(("additions".to_owned(), Tag::Compound(additions.clone())));
            }
            entries.push(("action".to_owned(), Tag::Compound(action)));
        }
        Ok(Tag::Compound(entries))
    }
}

/// One input control; its `key` names the entry in the click payload.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    key: String,
    control: Control,
}

#[derive(Debug, Clone, PartialEq)]
enum Control {
    Text {
        label: Tag,
        initial: String,
        max_length: u32,
        width: u32,
    },
    NumberRange {
        label: Tag,
        start: f32,
        end: f32,
        initial: Option<f32>,
        step: Option<f32>,
        width: u32,
    },
    Boolean {
        label: Tag,
        initial: bool,
    },
    SingleOption {
        label: Tag,
        /// `(id, display)`; `initial` is an index into this list.
        options: Vec<(String, Tag)>,
        initial: Option<usize>,
        width: u32,
    },
}

impl Input {
    /// Single-line text box (`max_length` defaults to vanilla's 32).
    pub fn text(key: &str, label: impl Into<Text>) -> Self {
        Self::new(
            key,
            Control::Text {
                label: label.into().0,
                initial: String::new(),
                max_length: 32,
                width: 200,
            },
        )
    }

    /// Slider over `start..=end`; the value arrives as a Float.
    pub fn number_range(key: &str, label: impl Into<Text>, start: f32, end: f32) -> Self {
        Self::new(
            key,
            Control::NumberRange {
                label: label.into().0,
                start,
                end,
                initial: None,
                step: None,
                width: 200,
            },
        )
    }

    /// Checkbox; the value arrives as a Byte (0/1).
    pub fn boolean(key: &str, label: impl Into<Text>) -> Self {
        Self::new(
            key,
            Control::Boolean {
                label: label.into().0,
                initial: false,
            },
        )
    }

    /// Cycle button over `(id, display)` options; the chosen id arrives as a String.
    pub fn single_option(key: &str, label: impl Into<Text>, options: Vec<(String, Tag)>) -> Self {
        Self::new(
            key,
            Control::SingleOption {
                label: label.into().0,
                options,
                initial: None,
                width: 200,
            },
        )
    }

    fn new(key: &str, control: Control) -> Self {
        Self {
            key: key.to_owned(),
            control,
        }
    }

    /// Initial value for text inputs.
    #[must_use]
    pub fn initial_text(mut self, value: &str) -> Self {
        if let Control::Text { initial, .. } = &mut self.control {
            *initial = value.to_owned();
        }
        self
    }

    #[must_use]
    pub fn max_length(mut self, value: u32) -> Self {
        if let Control::Text { max_length, .. } = &mut self.control {
            *max_length = value;
        }
        self
    }

    /// Initial slider value (must lie inside the range).
    #[must_use]
    pub fn initial_number(mut self, value: f32) -> Self {
        if let Control::NumberRange { initial, .. } = &mut self.control {
            *initial = Some(value);
        }
        self
    }

    /// Slider step; values snap to `initial + n * step` (vanilla `computeScaledValue`).
    #[must_use]
    pub fn step(mut self, value: f32) -> Self {
        if let Control::NumberRange { step, .. } = &mut self.control {
            *step = Some(value);
        }
        self
    }

    #[must_use]
    pub fn initial_bool(mut self, value: bool) -> Self {
        if let Control::Boolean { initial, .. } = &mut self.control {
            *initial = value;
        }
        self
    }

    /// Initially selected option, by id.
    #[must_use]
    pub fn initial_option(mut self, id: &str) -> Self {
        if let Control::SingleOption {
            options, initial, ..
        } = &mut self.control
        {
            *initial = options.iter().position(|(option, _)| option == id);
        }
        self
    }

    fn to_tag(&self) -> Result<Tag, DialogError> {
        if self.key.is_empty()
            || !self
                .key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return err(format!("input key {:?} must match [A-Za-z0-9_]+", self.key));
        }
        let mut entries = vec![("key".to_owned(), string(&self.key))];
        let mut push = |name: &str, value: Tag| entries.push((name.to_owned(), value));
        match &self.control {
            Control::Text {
                label,
                initial,
                max_length,
                width,
            } => {
                check_width(*width)?;
                if *max_length == 0 {
                    return err("text input max_length must be positive");
                }
                if initial.chars().count() > *max_length as usize {
                    return err("text input initial exceeds max_length");
                }
                push("type", string("minecraft:text"));
                push("label", label.clone());
                push("width", Tag::Int(*width as i32));
                push("initial", string(initial));
                push("max_length", Tag::Int(*max_length as i32));
            }
            Control::NumberRange {
                label,
                start,
                end,
                initial,
                step,
                width,
            } => {
                check_width(*width)?;
                if let Some(initial) = initial
                    && !(start.min(*end)..=start.max(*end)).contains(initial)
                {
                    return err("number_range initial is outside [start, end]");
                }
                if step.is_some_and(|step| step <= 0.0 || !step.is_finite()) {
                    return err("number_range step must be positive");
                }
                if !start.is_finite() || !end.is_finite() {
                    return err("number_range bounds must be finite");
                }
                push("type", string("minecraft:number_range"));
                push("label", label.clone());
                push("width", Tag::Int(*width as i32));
                push("start", Tag::Float(*start));
                push("end", Tag::Float(*end));
                if let Some(initial) = initial {
                    push("initial", Tag::Float(*initial));
                }
                if let Some(step) = step {
                    push("step", Tag::Float(*step));
                }
            }
            Control::Boolean { label, initial } => {
                push("type", string("minecraft:boolean"));
                push("label", label.clone());
                push("initial", Tag::Byte(i8::from(*initial)));
            }
            Control::SingleOption {
                label,
                options,
                initial,
                width,
            } => {
                check_width(*width)?;
                if options.is_empty() {
                    return err("single_option needs at least one option");
                }
                let list = options
                    .iter()
                    .enumerate()
                    .map(|(index, (id, display))| {
                        let mut option = vec![
                            ("id".to_owned(), string(id)),
                            ("display".to_owned(), display.clone()),
                        ];
                        if *initial == Some(index) {
                            option.push(("initial".to_owned(), Tag::Byte(1)));
                        }
                        Tag::Compound(option)
                    })
                    .collect();
                push("type", string("minecraft:single_option"));
                push("label", label.clone());
                push("width", Tag::Int(*width as i32));
                push("options", list_of(list));
            }
        }
        Ok(Tag::Compound(entries))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Notice(Button),
    Confirmation {
        yes: Button,
        no: Button,
    },
    MultiAction {
        actions: Vec<Button>,
        exit: Option<Button>,
        columns: u32,
    },
}

/// A dialog under construction. Defaults: closable with Escape, `pause = false`
/// (multiplayer dialogs must not freeze a singleplayer-hosted world), `after_action = close`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    title: Tag,
    body: Vec<Tag>,
    inputs: Vec<Input>,
    can_close_with_escape: bool,
    after_action: AfterAction,
    kind: Kind,
}

impl Dialog {
    fn new(title: Tag, kind: Kind) -> Self {
        Self {
            title,
            body: Vec::new(),
            inputs: Vec::new(),
            can_close_with_escape: true,
            after_action: AfterAction::Close,
            kind,
        }
    }

    /// One button; Escape runs that button's action.
    pub fn notice(title: impl Into<Text>, action: Button) -> Self {
        Self::new(title.into().0, Kind::Notice(action))
    }

    /// Yes/no pair; Escape runs the `no` action.
    pub fn confirmation(title: impl Into<Text>, yes: Button, no: Button) -> Self {
        Self::new(title.into().0, Kind::Confirmation { yes, no })
    }

    /// Button grid (2 columns by default) with an optional full-width exit button.
    pub fn multi_action(title: impl Into<Text>, actions: Vec<Button>) -> Self {
        Self::new(
            title.into().0,
            Kind::MultiAction {
                actions,
                exit: None,
                columns: 2,
            },
        )
    }

    /// Plain text paragraph (`plain_message`, 200 wide).
    #[must_use]
    pub fn text(mut self, contents: impl Into<Text>) -> Self {
        self.body.push(Tag::Compound(vec![
            ("type".to_owned(), string("minecraft:plain_message")),
            ("contents".to_owned(), contents.into().0),
        ]));
        self
    }

    #[must_use]
    pub fn input(mut self, input: Input) -> Self {
        self.inputs.push(input);
        self
    }

    /// Exit button of a `multi_action` dialog; Escape runs its action.
    #[must_use]
    pub fn exit(mut self, button: Button) -> Self {
        if let Kind::MultiAction { exit, .. } = &mut self.kind {
            *exit = Some(button);
        }
        self
    }

    #[must_use]
    pub fn columns(mut self, value: u32) -> Self {
        if let Kind::MultiAction { columns, .. } = &mut self.kind {
            *columns = value;
        }
        self
    }

    #[must_use]
    pub fn can_close_with_escape(mut self, value: bool) -> Self {
        self.can_close_with_escape = value;
        self
    }

    #[must_use]
    pub fn after_action(mut self, value: AfterAction) -> Self {
        self.after_action = value;
        self
    }

    /// The `Dialog.DIRECT_CODEC` NBT compound carried by `show_dialog`.
    pub fn into_tag(self) -> Result<Tag, DialogError> {
        let mut seen = std::collections::HashSet::new();
        for input in &self.inputs {
            if !seen.insert(input.key.as_str()) {
                return err(format!("duplicate input key {:?}", input.key));
            }
        }
        let (kind, extra): (&str, Vec<(String, Tag)>) = match &self.kind {
            Kind::Notice(action) => ("notice", vec![("action".to_owned(), action.to_tag()?)]),
            Kind::Confirmation { yes, no } => (
                "confirmation",
                vec![
                    ("yes".to_owned(), yes.to_tag()?),
                    ("no".to_owned(), no.to_tag()?),
                ],
            ),
            Kind::MultiAction {
                actions,
                exit,
                columns,
            } => {
                if actions.is_empty() {
                    return err("multi_action needs at least one action");
                }
                if *columns == 0 {
                    return err("multi_action columns must be positive");
                }
                let mut extra = vec![(
                    "actions".to_owned(),
                    list_of(
                        actions
                            .iter()
                            .map(Button::to_tag)
                            .collect::<Result<_, _>>()?,
                    ),
                )];
                if let Some(exit) = exit {
                    extra.push(("exit_action".to_owned(), exit.to_tag()?));
                }
                extra.push(("columns".to_owned(), Tag::Int(*columns as i32)));
                ("multi_action", extra)
            }
        };
        let mut entries = vec![
            ("type".to_owned(), string(&format!("minecraft:{kind}"))),
            ("title".to_owned(), self.title),
            (
                "can_close_with_escape".to_owned(),
                Tag::Byte(i8::from(self.can_close_with_escape)),
            ),
            ("pause".to_owned(), Tag::Byte(0)),
            ("after_action".to_owned(), string(self.after_action.name())),
        ];
        if !self.body.is_empty() {
            entries.push(("body".to_owned(), list_of(self.body)));
        }
        if !self.inputs.is_empty() {
            let inputs = self
                .inputs
                .iter()
                .map(Input::to_tag)
                .collect::<Result<_, _>>()?;
            entries.push(("inputs".to_owned(), list_of(inputs)));
        }
        entries.extend(extra);
        Ok(Tag::Compound(entries))
    }
}

fn string(value: &str) -> Tag {
    Tag::String(value.to_owned())
}

fn list_of(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: mc_nbt::tag_type::COMPOUND,
        elements,
    })
}

fn check_width(width: u32) -> Result<(), DialogError> {
    if (1..=1024).contains(&width) {
        Ok(())
    } else {
        err("width must be in 1..=1024")
    }
}

fn check_identifier(id: &str) -> Result<(), DialogError> {
    if id.contains(':') && mc_data::Identifier::parse(id).is_ok() {
        Ok(())
    } else {
        err(format!(
            "action id {id:?} must be a namespace:path identifier"
        ))
    }
}

/// Show `dialog` to `connection`, replacing whatever dialog it is looking at.
pub fn open(
    state: &mut TickState,
    connection: ConnectionId,
    dialog: Dialog,
) -> Result<(), DialogError> {
    let dialog = dialog.into_tag()?;
    state.push_player(connection, PlayerChange::ShowDialog { dialog });
    Ok(())
}

/// Close the dialog `connection` is looking at (for example after a server-side
/// answer to a [`AfterAction::WaitForResponse`] click).
pub fn clear(state: &mut TickState, connection: ConnectionId) {
    state.push_player(connection, PlayerChange::ClearDialog);
}

/// Typed reader for the compound a dialog click carries.
#[derive(Debug, Clone, Copy)]
pub struct ClickPayload<'a>(pub Option<&'a Tag>);

impl<'a> ClickPayload<'a> {
    fn get(&self, key: &str) -> Option<&'a Tag> {
        match self.0? {
            Tag::Compound(entries) => entries
                .iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, tag)| tag),
            _ => None,
        }
    }

    /// Text input or single-option id.
    pub fn string(&self, key: &str) -> Option<&'a str> {
        match self.get(key)? {
            Tag::String(value) => Some(value),
            _ => None,
        }
    }

    /// Boolean input.
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            Tag::Byte(value) => Some(*value != 0),
            _ => None,
        }
    }

    /// Number-range input (finite values only; a client may send anything).
    pub fn float(&self, key: &str) -> Option<f32> {
        match self.get(key)? {
            Tag::Float(value) if value.is_finite() => Some(*value),
            _ => None,
        }
    }
}

/// Deliver a click to its owner by the `<owner>` of `solaris:<owner>/<action>`.
/// Unknown ids are ignored (logged at debug): the client may be stale or hostile.
pub(crate) fn route_click(
    state: &mut TickState,
    connection: ConnectionId,
    id: &str,
    payload: Option<&Tag>,
    _changes: &mut ChangeSet,
) {
    let payload = ClickPayload(payload);
    let owner_action = id
        .strip_prefix("solaris:")
        .and_then(|rest| rest.split_once('/'));
    match owner_action {
        Some((probe::OWNER, action)) => probe::click(state, connection, action, payload),
        _ => {
            tracing::debug!(target: "dialog", connection = connection.0, id, "unrouted dialog click")
        }
    }
}

/// End-to-end test hook: with `SOLARIS_DIALOG_PROBE` set, every joining player is
/// shown a multi_action dialog (text input + number slider) and the click payload
/// is logged. Used by the real-client gate; inert otherwise.
pub(crate) mod probe {
    use super::{AfterAction, Button, ClickPayload, ConnectionId, Dialog, Input, PlayerChange};
    use crate::tick::TickState;
    use mc_nbt::Tag;

    pub(crate) const OWNER: &str = "dialog_probe";

    pub(crate) fn enabled() -> bool {
        static ENABLED: std::sync::LazyLock<bool> =
            std::sync::LazyLock::new(|| std::env::var_os("SOLARIS_DIALOG_PROBE").is_some());
        *ENABLED
    }

    pub(crate) fn dialog() -> Dialog {
        Dialog::multi_action(
            "Dialog probe",
            vec![
                Button::custom("Submit", "solaris:dialog_probe/submit")
                    .with("source", Tag::String("probe".into())),
                Button::custom("Second", "solaris:dialog_probe/second"),
            ],
        )
        .text("Server-driven dialog: type a name and pick a radius.")
        .input(
            Input::text("name", "Name")
                .initial_text("Ёлки")
                .max_length(16),
        )
        .input(
            Input::number_range("radius", "Radius", 1.0, 32.0)
                .initial_number(8.0)
                .step(1.0),
        )
        .input(Input::boolean("walls", "Walls"))
        .exit(Button::plain("Close"))
        .after_action(AfterAction::Close)
    }

    pub(crate) fn offer(state: &mut TickState, connection: ConnectionId) {
        if enabled() {
            let _ = super::open(state, connection, dialog());
        }
    }

    pub(super) fn click(
        state: &mut TickState,
        connection: ConnectionId,
        action: &str,
        payload: ClickPayload<'_>,
    ) {
        // Inert unless the probe is armed (tests exercise the route directly).
        if !(enabled() || cfg!(test)) {
            return;
        }
        let line = format!(
            "dialog probe: action={action} name={:?} radius={:?} walls={:?} source={:?}",
            payload.string("name"),
            payload.float("radius"),
            payload.bool("walls"),
            payload.string("source"),
        );
        tracing::info!(target: "dialog.probe", connection = connection.0, payload = ?payload.0, "{line}");
        state.push_player(connection, PlayerChange::Chat { message: line });
    }
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;

//! Every command Orbit has, for the Command palette: each a named action,
//! with its keyboard shortcut where it has one.

use gpui_kit::*;

use crate::{note_editor, window};

actions!(
    orbit,
    [
        /// The Quick switcher, for opening a note by name.
        QuickSwitcher,
        /// The Command palette.
        CommandPalette,
        /// Vault search, in the sidebar.
        SearchVault,
        /// A new folder at the vault root, named next.
        NewFolder,
        /// Rename the open note.
        RenameNote,
        /// Move the open note to another folder.
        MoveNote,
        /// Send the open note to the system trash.
        DeleteNote,
        /// Close the open note's tab.
        CloseTab,
        /// Show or hide the file tree.
        ToggleSidebar,
        /// Show or hide the Outline, Backlinks and Tags.
        ToggleRightPanel,
        /// Settings › Appearance.
        OpenSettings,
        /// The Welcome screen, for another vault.
        OpenAnotherVault,
        /// The unlock prompt, for protected folders.
        Unlock,
        /// Lock every unlocked protected folder.
        LockAll,
        /// The protect flow, for the open note's folder.
        ProtectFolder
    ]
);

pub fn init(cx: &mut App) {
    let window = Some(window::KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-o", QuickSwitcher, window),
        KeyBinding::new("ctrl-p", CommandPalette, window),
        KeyBinding::new("ctrl-shift-f", SearchVault, window),
        KeyBinding::new("ctrl-w", CloseTab, window),
        KeyBinding::new("alt-1", ToggleSidebar, window),
        KeyBinding::new("alt-2", ToggleRightPanel, window),
        KeyBinding::new("ctrl-,", OpenSettings, window),
        KeyBinding::new("ctrl-shift-l", LockAll, window),
    ]);
}

/// A command in the palette.
pub struct Command {
    pub name: &'static str,
    /// The key context its shortcut is bound in.
    pub context: &'static str,
    /// It acts on the open note, so it's only listed while one is.
    pub needs_note: bool,
    action: fn() -> Box<dyn Action>,
}

impl Command {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }
}

/// The palette's commands, in the order it lists them.
pub fn all() -> &'static [Command] {
    const WINDOW: &str = window::KEY_CONTEXT;
    const NOTE: &str = note_editor::KEY_CONTEXT;
    macro_rules! command {
        ($name:literal, $action:expr, $context:expr) => {
            command!($name, $action, $context, false)
        };
        ($name:literal, $action:expr, $context:expr, $needs_note:expr) => {
            Command {
                name: $name,
                context: $context,
                needs_note: $needs_note,
                action: || Box::new($action),
            }
        };
    }
    const COMMANDS: &[Command] = &[
        command!("Open a note", QuickSwitcher, WINDOW),
        command!("Search the vault", SearchVault, WINDOW),
        command!("New note", crate::file_actions::NewNote, WINDOW),
        command!("New folder", NewFolder, WINDOW),
        command!("Save", note_editor::Save, NOTE, true),
        command!("Rename note", RenameNote, WINDOW, true),
        command!("Move note", MoveNote, WINDOW, true),
        command!("Delete note", DeleteNote, WINDOW, true),
        command!("Close tab", CloseTab, WINDOW, true),
        command!("Bold", note_editor::ToggleBold, NOTE, true),
        command!("Italic", note_editor::ToggleItalic, NOTE, true),
        command!("Insert link", note_editor::InsertLink, NOTE, true),
        command!("Toggle sidebar", ToggleSidebar, WINDOW),
        command!("Toggle right panel", ToggleRightPanel, WINDOW),
        command!("Open settings", OpenSettings, WINDOW),
        command!("Open another vault", OpenAnotherVault, WINDOW),
        command!("Unlock…", Unlock, WINDOW),
        command!("Lock all protected folders", LockAll, WINDOW),
        command!("Protect a folder…", ProtectFolder, WINDOW),
    ];
    COMMANDS
}

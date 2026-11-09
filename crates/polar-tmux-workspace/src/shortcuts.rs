//! Station action catalog. Native shells supply focus and physical key events;
//! this table owns action identity, suffix, copy and repetition policy.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[repr(u32)]
pub enum StationShortcutAction {
    TogglePanel = 1,
    ToggleMode = 2,
    PreviousPane = 3,
    NextPane = 4,
    NumberedPane = 5,
    NewShell = 6,
    NewCodex = 7,
    NewWorkspace = 8,
    RenameWorkspace = 9,
    RefreshSessions = 10,
    ClosePane = 11,
    OpenIdeaTree = 12,
    ResumeCodex = 13,
    OpenInquiry = 14,
    InsertIdeaNode = 15,
    EditIdeaInstruction = 16,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct StationShortcut {
    pub action: StationShortcutAction,
    pub number: u8,
    pub key: &'static str,
    pub label: &'static str,
    pub mac_keycode: u16,
    pub linux_keyval: u32,
    pub repeatable: bool,
}

pub const STATION_SHORTCUTS: &[StationShortcut] = &[
    key(
        StationShortcutAction::TogglePanel,
        "S",
        "Control Center",
        1,
        b'S' as u32,
    ),
    key(
        StationShortcutAction::ToggleMode,
        "M",
        "Monitor / Focus",
        46,
        b'M' as u32,
    ),
    StationShortcut {
        repeatable: true,
        ..key(
            StationShortcutAction::PreviousPane,
            "↑",
            "Previous Pane",
            126,
            0xff52,
        )
    },
    StationShortcut {
        repeatable: true,
        ..key(
            StationShortcutAction::NextPane,
            "↓",
            "Next Pane",
            125,
            0xff54,
        )
    },
    digit(1, 18),
    digit(2, 19),
    digit(3, 20),
    digit(4, 21),
    digit(5, 23),
    digit(6, 22),
    digit(7, 26),
    digit(8, 28),
    digit(9, 25),
    key(
        StationShortcutAction::NewShell,
        "J",
        "New Shell in this Tab",
        38,
        b'J' as u32,
    ),
    key(
        StationShortcutAction::NewCodex,
        "K",
        "New Codex in this Tab",
        40,
        b'K' as u32,
    ),
    key(
        StationShortcutAction::NewWorkspace,
        "A",
        "New Workspace",
        0,
        b'A' as u32,
    ),
    key(
        StationShortcutAction::RenameWorkspace,
        "R",
        "Rename Workspace",
        15,
        b'R' as u32,
    ),
    key(
        StationShortcutAction::RefreshSessions,
        "U",
        "Refresh Sessions",
        32,
        b'U' as u32,
    ),
    key(
        StationShortcutAction::ClosePane,
        "X",
        "Close current Pane",
        7,
        b'X' as u32,
    ),
    key(StationShortcutAction::OpenIdeaTree, "B", "Open Idea Tree in this Tab", 11, b'B' as u32),
    key(StationShortcutAction::ResumeCodex, "L", "Resume Codex Session", 37, b'L' as u32),
    key(StationShortcutAction::OpenInquiry, "Y", "Handle Inquiry", 16, b'Y' as u32),
    key(StationShortcutAction::InsertIdeaNode, "I", "Insert node", 34, b'I' as u32),
    key(StationShortcutAction::EditIdeaInstruction, "E", "Edit instruction", 14, b'E' as u32),
];

const fn key(
    action: StationShortcutAction,
    key: &'static str,
    label: &'static str,
    mac_keycode: u16,
    linux_keyval: u32,
) -> StationShortcut {
    StationShortcut {
        action,
        number: 0,
        key,
        label,
        mac_keycode,
        linux_keyval,
        repeatable: false,
    }
}
const fn digit(number: u8, mac_keycode: u16) -> StationShortcut {
    StationShortcut {
        number,
        ..key(
            StationShortcutAction::NumberedPane,
            match number {
                1 => "1",
                2 => "2",
                3 => "3",
                4 => "4",
                5 => "5",
                6 => "6",
                7 => "7",
                8 => "8",
                _ => "9",
            },
            "Choose numbered Pane",
            mac_keycode,
            b'0' as u32 + number as u32,
        )
    }
}

pub fn station_shortcut(platform: u32, native_key: u32) -> Option<&'static StationShortcut> {
    STATION_SHORTCUTS.iter().find(|entry| match platform {
        1 => u32::from(entry.mac_keycode) == native_key,
        2 => entry.linux_keyval == native_key,
        _ => false,
    })
}




/// Pure gesture policy. OS capture and intent execution are external.
#[derive(Default)]
pub struct DictateGesture {
    down: bool,
    release_stops: bool,
    session_toggle: Option<bool>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum GestureIntent { None = 0, Start = 1, Stop = 2 }
#[derive(Clone, Copy, Debug)]
pub enum ShortcutEvent { Press, Release, Activate, Cancel }
impl DictateGesture {
    pub fn interpret(&mut self, event: ShortcutEvent, toggle: bool, recording: bool, can_start: bool) -> GestureIntent {
        match event {
            ShortcutEvent::Cancel => { self.down = false; self.release_stops = false; self.session_toggle = None; GestureIntent::None }
            ShortcutEvent::Release => {
                self.down = false;
                if std::mem::take(&mut self.release_stops) { GestureIntent::Stop } else { GestureIntent::None }
            }
            ShortcutEvent::Press | ShortcutEvent::Activate => {
                // Activate is a discrete nonrepeat OS activation, not a raw key-down.
                if matches!(event, ShortcutEvent::Press) && self.down { return GestureIntent::None; }
                self.down = matches!(event, ShortcutEvent::Press);
                let toggle = if recording { self.session_toggle.unwrap_or(toggle) } else { toggle };
                if toggle && recording { self.release_stops = false; return GestureIntent::Stop; }
                if !recording && can_start {
                    self.session_toggle = Some(toggle);
                    self.release_stops = !toggle;
                    GestureIntent::Start
                } else { GestureIntent::None }
            }
        }
    }
}


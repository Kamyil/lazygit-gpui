//! The messages exchanged with lazygit running in desktop mode (see
//! pkg/gui/desktop.go, which documents the protocol). Each message is one line
//! of JSON.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Incoming {
    Frame(Frame),
    External(External),
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Frame {
    pub seq: u64,
    pub sync: u64,
    pub repo: String,
    pub focus: String,
    pub context: String,
    pub context_kind: String,
    pub side_windows: Vec<Window>,
    pub main_windows: Vec<Window>,
    pub popups: Vec<String>,
    pub views: HashMap<String, ViewMeta>,
    pub lines: HashMap<String, Vec<Vec<Run>>>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Window {
    pub name: String,
    pub view: String,
    pub tab_views: Vec<String>,
    pub tab_titles: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewMeta {
    pub title: String,
    pub subtitle: String,
    pub footer: String,
    pub tabs: Vec<String>,
    pub tab_index: usize,
    pub window: String,
    pub context: String,
    pub kind: String,
    pub focused: bool,
    pub line_count: usize,
    pub loading: bool,
    pub origin: usize,
    pub cursor: i64,
    pub range_start: i64,
    pub range_end: i64,
    pub highlight: bool,
    pub inactive: bool,
    pub selection_width: usize,
    pub gutter: Vec<usize>,
    pub gutter_marker: String,
    pub editable: bool,
    pub input: Option<Input>,
    pub fg: u32,
    pub search: Option<Vec<i64>>,
}

impl ViewMeta {
    /// The buffer lines the view shows as selected, if it shows a selection.
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.highlight && self.range_start >= 0 && self.range_end >= self.range_start)
            .then(|| (self.range_start as usize, self.range_end as usize))
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Input {
    pub cursor_line: usize,
    pub cursor_byte: usize,
    pub mask: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Run {
    #[serde(rename = "t")]
    pub text: String,
    #[serde(rename = "f")]
    pub fg: u32,
    #[serde(rename = "b")]
    pub bg: u32,
    #[serde(rename = "a")]
    pub attributes: u8,
    #[serde(rename = "h")]
    pub hyperlink: String,
}

pub const BOLD: u8 = 1;
pub const UNDERLINE: u8 = 2;
pub const REVERSE: u8 = 4;
pub const DIM: u8 = 8;
pub const ITALIC: u8 = 16;
pub const STRIKETHROUGH: u8 = 32;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct External {
    pub command: String,
    pub running: bool,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Outgoing {
    Key {
        key: String,
    },
    Text {
        text: String,
    },
    Paste {
        text: String,
    },
    Click {
        view: String,
        x: usize,
        y: usize,
        double: bool,
        extend: bool,
    },
    Tab {
        window: String,
        index: usize,
    },
    Load {
        view: String,
        lines: usize,
    },
    Focus {
        focused: bool,
    },
    /// Asks for a frame tagged with this id once lazygit has handled all
    /// earlier input. Only test drivers wait like that; the app doesn't.
    #[allow(dead_code)]
    Sync {
        id: u64,
    },
    CancelExternal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_frame() {
        let line = r#"{"type":"frame","seq":3,"repo":"/r","focus":"files","context":"files","contextKind":"side",
            "sideWindows":[{"name":"files","view":"files","tabViews":["files","worktrees"],"tabTitles":["Files","Worktrees"]}],
            "mainWindows":[{"name":"main","view":"main"}],"popups":[],
            "views":{"files":{"title":"[2]Files","window":"files","kind":"side","focused":true,"lineCount":2,
                "origin":0,"cursor":1,"rangeStart":1,"rangeEnd":1,"highlight":true,"tabIndex":0}},
            "lines":{"files":[[{"t":"▼ /"}],[{"t":" M","f":2},{"t":" a.txt","a":1}]]}}"#;
        let Incoming::Frame(frame) = serde_json::from_str(line).unwrap() else {
            panic!("not a frame")
        };
        assert_eq!(frame.side_windows[0].tab_titles, ["Files", "Worktrees"]);
        let files = &frame.views["files"];
        assert_eq!(files.selection(), Some((1, 1)));
        assert_eq!(frame.lines["files"][1][0].fg, 2);
        assert_eq!(frame.lines["files"][1][1].attributes, BOLD);
    }

    #[test]
    fn encodes_input() {
        let click = Outgoing::Click {
            view: "files".into(),
            x: 0,
            y: 4,
            double: true,
            extend: false,
        };
        assert_eq!(
            serde_json::to_string(&click).unwrap(),
            r#"{"type":"click","view":"files","x":0,"y":4,"double":true,"extend":false}"#
        );
        assert_eq!(
            serde_json::to_string(&Outgoing::CancelExternal).unwrap(),
            r#"{"type":"cancelExternal"}"#
        );
    }
}

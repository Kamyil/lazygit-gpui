//! The frontend's copy of lazygit's views, kept up to date from frames, and the
//! scrolling that each frame calls for.

use crate::protocol::{
    BOLD, DIM, External, Frame, ITALIC, REVERSE, Run, STRIKETHROUGH, UNDERLINE, ViewMeta, Window,
};
use crate::theme;
use gpui::{FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle, UnderlineStyle, px};
use std::{collections::HashMap, ops::Range, rc::Rc};

/// A line of a view, prepared for drawing.
pub struct Line {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// The color of the line's first run, which tells added and removed diff
    /// lines apart.
    pub lead_fg: u32,
}

impl Line {
    pub fn from_runs(runs: &[Run]) -> Self {
        let mut text = String::new();
        let mut highlights = Vec::new();
        for run in runs {
            let start = text.len();
            text.push_str(&run.text);
            if let Some(style) = highlight_style(run) {
                highlights.push((start..text.len(), style));
            }
        }
        Line {
            text: text.into(),
            highlights,
            lead_fg: runs
                .iter()
                .find(|run| !run.text.trim().is_empty())
                .map_or(0, |run| run.fg),
        }
    }
}

fn highlight_style(run: &Run) -> Option<HighlightStyle> {
    let (mut fg, mut bg) = (theme::terminal_color(run.fg), theme::terminal_color(run.bg));
    if run.attributes & REVERSE != 0 {
        (fg, bg) = (Some(bg.unwrap_or(theme::PAPER)), Some(fg.unwrap_or(theme::INK)));
    }
    let style = HighlightStyle {
        color: fg.map(theme::hsla),
        background_color: bg.map(theme::hsla),
        font_weight: (run.attributes & BOLD != 0).then_some(FontWeight::BOLD),
        font_style: (run.attributes & ITALIC != 0).then_some(FontStyle::Italic),
        underline: (run.attributes & UNDERLINE != 0 || !run.hyperlink.is_empty()).then(|| {
            UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            }
        }),
        strikethrough: (run.attributes & STRIKETHROUGH != 0).then(|| StrikethroughStyle {
            thickness: px(1.),
            ..Default::default()
        }),
        fade_out: (run.attributes & DIM != 0).then_some(0.4),
    };
    (style != HighlightStyle::default()).then_some(style)
}

pub struct ViewState {
    pub meta: ViewMeta,
    pub lines: Rc<Vec<Line>>,
}

#[derive(Debug, PartialEq)]
pub enum Scroll {
    /// Scroll as little as needed to show this line.
    Reveal(usize),
    /// Show this line at the top.
    Top(usize),
}

#[derive(Default)]
pub struct Model {
    pub repo: String,
    pub focus: String,
    pub context: String,
    pub side_windows: Vec<Window>,
    pub main_windows: Vec<Window>,
    pub popups: Vec<String>,
    pub views: HashMap<String, ViewState>,
    pub external: Option<External>,
    pub notice: Option<String>,
    pub received_frame: bool,
}

impl Model {
    /// Applies a frame, returning how to scroll the views whose position or
    /// content changed.
    pub fn apply(&mut self, mut frame: Frame) -> Vec<(String, Scroll)> {
        let mut scrolls = Vec::new();
        let mut views = HashMap::with_capacity(frame.views.len());
        for (name, meta) in frame.views {
            let old = self.views.remove(&name);
            let (lines, replaced) = match (frame.lines.remove(&name), &old) {
                (Some(runs), old) => {
                    let lines: Vec<Line> = runs.iter().map(|line| Line::from_runs(line)).collect();
                    let replaced = old.as_ref().is_none_or(|old| !extends(&old.lines, &lines));
                    (Rc::new(lines), replaced)
                }
                (None, Some(old)) => (old.lines.clone(), false),
                (None, None) => (Rc::new(Vec::new()), true),
            };
            if let Some(scroll) = scroll_for(old.as_ref().map(|old| &old.meta), &meta, replaced) {
                scrolls.push((name.clone(), scroll));
            }
            views.insert(name, ViewState { meta, lines });
        }
        self.views = views;
        self.repo = frame.repo;
        self.focus = frame.focus;
        self.context = frame.context;
        self.side_windows = frame.side_windows;
        self.main_windows = frame.main_windows;
        self.popups = frame.popups;
        self.received_frame = true;
        scrolls
    }

    pub fn view(&self, name: &str) -> Option<&ViewState> {
        self.views.get(name)
    }

    /// Whether keyboard input goes to a text field.
    pub fn editing(&self) -> bool {
        self.view(&self.focus).is_some_and(|view| view.meta.editable)
    }
}

/// Whether new content is old content with lines added at the end, as when
/// lazygit reads more of a long diff, rather than different content.
fn extends(old: &[Line], new: &[Line]) -> bool {
    if old.is_empty() || new.len() < old.len() {
        return old.is_empty() && new.is_empty();
    }
    let same = |i: usize| old[i].text == new[i].text;
    same(0) && same(old.len() / 2) && same(old.len() - 1)
}

fn scroll_for(old: Option<&ViewMeta>, new: &ViewMeta, replaced: bool) -> Option<Scroll> {
    let cursor = (new.highlight && new.cursor >= 0).then_some(new.cursor as usize);
    let old_cursor = old.and_then(|old| (old.highlight && old.cursor >= 0).then_some(old.cursor));
    let origin_moved = old.is_none_or(|old| old.origin != new.origin);

    if let Some(cursor) = cursor {
        // The cursor is the moving end of a range selection, too.
        let moved = replaced || old_cursor != Some(new.cursor);
        return moved.then_some(Scroll::Reveal(cursor));
    }
    (replaced || origin_moved).then_some(Scroll::Top(new.origin))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Vec<Run> {
        vec![Run {
            text: text.into(),
            ..Default::default()
        }]
    }

    fn frame(views: Vec<(&str, ViewMeta, Option<Vec<&str>>)>) -> Frame {
        let mut frame = Frame::default();
        for (name, meta, lines) in views {
            frame.views.insert(name.into(), meta);
            if let Some(lines) = lines {
                frame.lines.insert(name.into(), lines.into_iter().map(run).collect());
            }
        }
        frame
    }

    fn list(cursor: i64) -> ViewMeta {
        ViewMeta {
            highlight: true,
            cursor,
            range_start: cursor,
            range_end: cursor,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_content_that_was_not_resent() {
        let mut model = Model::default();
        model.apply(frame(vec![("files", list(0), Some(vec!["a", "b"]))]));
        model.apply(frame(vec![("files", list(1), None)]));
        let files = model.view("files").unwrap();
        assert_eq!(files.lines.len(), 2);
        assert_eq!(files.meta.cursor, 1);
    }

    #[test]
    fn drops_views_that_are_no_longer_visible() {
        let mut model = Model::default();
        model.apply(frame(vec![("menu", list(0), Some(vec!["x"]))]));
        model.apply(frame(vec![]));
        assert!(model.view("menu").is_none());
    }

    #[test]
    fn reveals_a_moved_selection() {
        let mut model = Model::default();
        model.apply(frame(vec![("commits", list(0), Some(vec!["a", "b", "c"]))]));
        let scrolls = model.apply(frame(vec![("commits", list(2), None)]));
        assert_eq!(scrolls, [("commits".to_string(), Scroll::Reveal(2))]);
        assert!(model.apply(frame(vec![("commits", list(2), None)])).is_empty());
    }

    #[test]
    fn follows_the_origin_of_views_without_selection() {
        let mut model = Model::default();
        let main = |origin| ViewMeta {
            origin,
            ..Default::default()
        };
        model.apply(frame(vec![("main", main(0), Some(vec!["a", "b"]))]));
        // More of the same diff was read: stay where the user scrolled to.
        assert!(model.apply(frame(vec![("main", main(0), Some(vec!["a", "b", "c"]))])).is_empty());
        // A different diff: start at its top.
        let scrolls = model.apply(frame(vec![("main", main(0), Some(vec!["x"]))]));
        assert_eq!(scrolls, [("main".to_string(), Scroll::Top(0))]);
        let scrolls = model.apply(frame(vec![("main", main(1), None)]));
        assert_eq!(scrolls, [("main".to_string(), Scroll::Top(1))]);
    }

    #[test]
    fn prepares_styled_lines() {
        let line = Line::from_runs(&[
            Run {
                text: " M".into(),
                fg: 2,
                ..Default::default()
            },
            Run {
                text: " a.txt".into(),
                attributes: BOLD,
                ..Default::default()
            },
        ]);
        assert_eq!(line.text.as_ref(), " M a.txt");
        assert_eq!(line.highlights.len(), 2);
        assert_eq!(line.highlights[1].0, 2..8);
        assert_eq!(line.lead_fg, 2);
    }
}

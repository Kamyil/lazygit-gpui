mod backend;

use backend::{BackendEvent, Pane, Snapshot};
use futures::StreamExt;
use gpui::{
    App, Application, Bounds, Context, FocusHandle, KeyDownEvent, Keystroke, ScrollStrategy,
    SharedString, UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, rgb, size, uniform_list,
};
use std::{path::PathBuf, sync::mpsc::Sender};

const PAPER: u32 = 0x181d21;
const SIDEBAR: u32 = 0x1e252a;
const RULE: u32 = 0x343e45;
const INK: u32 = 0xe0e6e6;
const MUTED: u32 = 0x9baeb6;
const ACCENT: u32 = 0xe4bd79;

struct Workbench {
    focus: FocusHandle,
    keys: Sender<String>,
    snapshot: Option<Snapshot>,
    status: SharedString,
    repo: SharedString,
    files_scroll: UniformListScrollHandle,
    main_scroll: UniformListScrollHandle,
    secondary_scroll: UniformListScrollHandle,
    other_scroll: UniformListScrollHandle,
}

impl Workbench {
    fn apply(&mut self, next: Snapshot) {
        let old = self.snapshot.as_ref();
        let selection_changed = old.is_none_or(|old| {
            old.files.selected != next.files.selected
                || old.files.lines.get(old.files.selected)
                    != next.files.lines.get(next.files.selected)
        });
        if selection_changed && next.files.selected < next.files.lines.len() {
            self.files_scroll
                .scroll_to_item(next.files.selected, ScrollStrategy::Top);
        }
        for (handle, before, after) in [
            (&self.main_scroll, old.map(|s| &s.main), Some(&next.main)),
            (
                &self.secondary_scroll,
                old.and_then(|s| s.secondary.as_ref()),
                next.secondary.as_ref(),
            ),
            (
                &self.other_scroll,
                old.and_then(|s| s.other.as_ref()),
                next.other.as_ref(),
            ),
        ] {
            if let Some(pane) = after {
                if !pane.lines.is_empty()
                    && (selection_changed
                        || before.is_none_or(|p| p.origin != pane.origin || p.title != pane.title))
                {
                    handle.scroll_to_item_strict(
                        pane.origin.min(pane.lines.len() - 1),
                        ScrollStrategy::Top,
                    );
                }
            }
        }
        self.status = "Keyboard → LazyGit".into();
        self.snapshot = Some(next);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = lazygit_label(&event.keystroke) {
            let _ = self.keys.send(key);
            cx.stop_propagation();
        }
    }
}

fn lazygit_label(stroke: &Keystroke) -> Option<String> {
    let m = stroke.modifiers;
    if m.platform || m.function {
        return None;
    }
    let modified = m.control || m.alt;
    let key = match stroke.key.as_str() {
        "escape" => "esc",
        "pageup" => "pgup",
        "pagedown" => "pgdown",
        "tab" if m.shift && !modified => "backtab",
        " " => "space",
        key if !modified && key.chars().count() == 1 => stroke.key_char.as_deref().unwrap_or(key),
        key => key,
    };
    let key = match key {
        " " => "space",
        "-" => "minus",
        "+" => "plus",
        key => key,
    };
    let shift = m.shift
        && (modified || (key.chars().count() > 1 && !matches!(key, "backtab" | "minus" | "plus")));
    if !modified && !shift && key.chars().count() == 1 {
        return Some(key.to_owned());
    }
    Some(format!(
        "<{}{}{}{key}>",
        if m.control { "ctrl+" } else { "" },
        if m.alt { "alt+" } else { "" },
        if shift { "shift+" } else { "" }
    ))
}

#[derive(Clone, Copy)]
enum Panel {
    Files,
    Main,
    Secondary,
    Other,
}

impl Panel {
    fn pane(self, state: &Snapshot) -> Option<&Pane> {
        match self {
            Self::Files => Some(&state.files),
            Self::Main => Some(&state.main),
            Self::Secondary => state.secondary.as_ref(),
            Self::Other => state.other.as_ref(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Main => "Diff",
            Self::Secondary => "Staged diff",
            Self::Other => "LazyGit",
        }
    }

    fn diff(self) -> bool {
        matches!(self, Self::Main | Self::Secondary)
    }

    fn view(self, app: &Workbench, cx: &mut Context<Workbench>) -> impl IntoElement {
        let snapshot = app.snapshot.as_ref();
        let pane = snapshot.and_then(|s| self.pane(s));
        let title = pane
            .filter(|p| !p.title.is_empty())
            .map_or_else(|| self.label().into(), |p| p.title.clone());
        let count = pane.map_or(0, |p| p.lines.len());
        let (scroll, focused) = match self {
            Self::Files => (
                &app.files_scroll,
                snapshot.is_some_and(|s| s.focus == "files"),
            ),
            Self::Main => (
                &app.main_scroll,
                snapshot.is_some_and(|s| s.focus == "main"),
            ),
            Self::Secondary => (
                &app.secondary_scroll,
                snapshot.is_some_and(|s| s.focus == "secondary"),
            ),
            Self::Other => (&app.other_scroll, true),
        };
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex_none()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(RULE))
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(if focused { ACCENT } else { MUTED }))
                    .child(title),
            )
            .when(count == 0, |d| {
                d.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("Nothing to display"),
                )
            })
            .when(count > 0, |d| {
                d.child(
                    uniform_list(
                        self.label(),
                        count,
                        cx.processor(move |app, range: std::ops::Range<usize>, _, _| {
                            let pane = app.snapshot.as_ref().and_then(|s| self.pane(s));
                            range
                                .filter_map(|i| {
                                    let pane = pane?;
                                    let text = pane.lines.get(i)?;
                                    let selected = !self.diff() && i == pane.selected;
                                    let (fg, bg) = match text.as_ref() {
                                        _ if selected => (INK, 0x354851),
                                        s if self.diff() && s.starts_with("@@") => {
                                            (0x9dc9e4, 0x202f3c)
                                        }
                                        s if self.diff()
                                            && (s.starts_with("diff ")
                                                || s.starts_with("+++ ")
                                                || s.starts_with("--- ")) =>
                                        {
                                            (MUTED, PAPER)
                                        }
                                        s if self.diff() && s.starts_with('+') => {
                                            (0xade0b6, 0x22362b)
                                        }
                                        s if self.diff() && s.starts_with('-') => {
                                            (0xf1b6b1, 0x3b292c)
                                        }
                                        _ => (INK, if self.diff() { PAPER } else { SIDEBAR }),
                                    };
                                    Some(
                                        div()
                                            .h(px(22.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .bg(rgb(bg))
                                            .text_color(rgb(fg))
                                            .when(self.diff(), |d| {
                                                d.child(
                                                    div()
                                                        .w(px(44.))
                                                        .flex_none()
                                                        .text_color(rgb(MUTED))
                                                        .child(format!("{:>4}", i + 1)),
                                                )
                                            })
                                            .child(text.clone()),
                                    )
                                })
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(scroll.clone())
                    .flex_1()
                    .min_h_0(),
                )
            })
    }
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .font_family("Menlo")
            .text_size(px(12.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .h(px(46.))
                    .border_b_1()
                    .border_color(rgb(RULE))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(rgb(ACCENT))
                            .child("LAZYGIT / WORKTREE"),
                    )
                    .child(
                        div()
                            .text_color(rgb(MUTED))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(self.repo.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(310.))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .bg(rgb(SIDEBAR))
                            .border_r_1()
                            .border_color(rgb(RULE))
                            .child(Panel::Files.view(self, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(
                                self.snapshot.as_ref().is_some_and(|s| s.other.is_some()),
                                |d| {
                                    d.child(
                                        div()
                                            .h(px(150.))
                                            .flex_none()
                                            .flex()
                                            .flex_col()
                                            .bg(rgb(SIDEBAR))
                                            .border_b_1()
                                            .border_color(rgb(RULE))
                                            .child(Panel::Other.view(self, cx)),
                                    )
                                },
                            )
                            .child(Panel::Main.view(self, cx))
                            .when(
                                self.snapshot
                                    .as_ref()
                                    .is_some_and(|s| s.secondary.is_some()),
                                |d| {
                                    d.child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .flex()
                                            .flex_col()
                                            .border_t_1()
                                            .border_color(rgb(RULE))
                                            .child(Panel::Secondary.view(self, cx)),
                                    )
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .h(px(28.))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(rgb(RULE))
                    .text_color(rgb(MUTED))
                    .child(self.status.clone()),
            )
    }
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let backend = args
        .next()
        .map(PathBuf::from)
        .ok_or("Usage: lazygit-gpui /path/to/patched/lazygit [repository]")?;
    let backend = backend
        .canonicalize()
        .map_err(|e| format!("Resolve backend: {e}"))?;
    let repo = args
        .next()
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|e| e.to_string())?);
    let repo = repo
        .canonicalize()
        .map_err(|e| format!("Resolve repository: {e}"))?;
    let (keys, mut events) = backend::launch(backend, &repo)?;
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1160.), px(760.)), cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window);
                cx.new(|cx: &mut Context<Workbench>| {
                    cx.spawn(async move |this, cx| {
                        while let Some(event) = events.next().await {
                            let result = this.update(cx, |app, cx| {
                                match event {
                                    BackendEvent::Frame(frame) => app.apply(frame),
                                    BackendEvent::Closed => cx.quit(),
                                    BackendEvent::Error(error) => app.status = error.into(),
                                }
                                cx.notify();
                            });
                            if result.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    Workbench {
                        focus,
                        keys,
                        snapshot: None,
                        status: "Starting LazyGit…".into(),
                        repo: repo.to_string_lossy().into_owned().into(),
                        files_scroll: Default::default(),
                        main_scroll: Default::default(),
                        secondary_scroll: Default::default(),
                        other_scroll: Default::default(),
                    }
                })
            },
        )
        .expect("open GPUI window");
        cx.activate(true);
    });
    Ok(())
}

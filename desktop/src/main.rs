mod backend;
mod keys;
mod model;
mod protocol;
mod theme;

use backend::{Backend, BackendEvent};
use futures::StreamExt;
use gpui::{
    AnyElement, App, Application, Bounds, Context, ElementId, FocusHandle,
    FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, ScrollStrategy, SharedString,
    StyledText, Subscription, UniformListScrollHandle, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, relative, rgb, rgba, size, uniform_list,
};
use keys::KeyAction;
use model::{Model, Scroll};
use protocol::{Incoming, Outgoing, Window as SideWindow};
use std::{collections::HashMap, ops::Range, path::PathBuf};
use theme::*;

const ROW: f32 = 20.;
/// How many lines beyond the end of what is loaded to ask lazygit for, when
/// scrolling gets close to the end of a view that is still loading.
const LOAD_AHEAD: usize = 2000;

struct Workbench {
    focus: FocusHandle,
    backend: Backend,
    model: Model,
    status: SharedString,
    scroll_handles: HashMap<String, UniformListScrollHandle>,
    requested_lines: HashMap<String, usize>,
    _activation: Subscription,
}

impl Workbench {
    fn handle(&mut self, event: BackendEvent, cx: &mut Context<Self>) {
        match event {
            BackendEvent::Message(Incoming::Frame(frame)) => {
                for (view, scroll) in self.model.apply(frame) {
                    let handle = self.scroll_handle(&view);
                    match scroll {
                        Scroll::Reveal(line) => handle.scroll_to_item(line, ScrollStrategy::Center),
                        Scroll::Top(line) => handle.scroll_to_item_strict(line, ScrollStrategy::Top),
                    }
                }
                let views = &self.model.views;
                self.scroll_handles.retain(|name, _| views.contains_key(name));
                self.requested_lines.retain(|name, _| views.contains_key(name));
            }
            BackendEvent::Message(Incoming::External(external)) => {
                if !external.running && !external.error.is_empty() {
                    self.model.notice = Some(external.error.clone());
                } else {
                    self.model.notice = None;
                }
                self.model.external = external.running.then_some(external);
            }
            BackendEvent::Closed => cx.quit(),
            BackendEvent::Error(error) => self.status = error.into(),
        }
        cx.notify();
    }

    fn scroll_handle(&mut self, view: &str) -> UniformListScrollHandle {
        self.scroll_handles.entry(view.to_owned()).or_default().clone()
    }

    fn send(&self, message: Outgoing) {
        self.backend.send(message);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = keys::key_action(&event.keystroke, self.model.editing()) else {
            return;
        };
        cx.stop_propagation();
        match action {
            KeyAction::Send(message) => self.send(message),
            KeyAction::Paste => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.send(Outgoing::Paste { text });
                }
            }
            KeyAction::Quit => cx.quit(),
        }
    }

    fn click(&mut self, view: &str, line: usize, event: &MouseDownEvent) {
        self.send(Outgoing::Click {
            view: view.into(),
            x: 0,
            y: line,
            double: event.click_count >= 2,
            extend: event.modifiers.shift,
        });
    }

    /// Asks lazygit to read further into a view whose content is still
    /// loading, once scrolling gets near the end of what it has read.
    fn load_more_if_needed(&mut self, name: &str, visible: &Range<usize>) {
        let Some(view) = self.model.view(name) else {
            return;
        };
        let loaded = view.lines.len();
        if !view.meta.loading || visible.end + LOAD_AHEAD / 2 < loaded {
            return;
        }
        let wanted = loaded + LOAD_AHEAD;
        if self.requested_lines.get(name).is_some_and(|&requested| requested >= wanted) {
            return;
        }
        self.requested_lines.insert(name.to_owned(), wanted);
        self.send(Outgoing::Load {
            view: name.into(),
            lines: wanted,
        });
    }

    fn rows(&mut self, name: &str, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.load_more_if_needed(name, &range);
        let Some(view) = self.model.view(name) else {
            return Vec::new();
        };
        let meta = &view.meta;
        let selection = meta.selection();
        let active = !meta.inactive;
        let edge_selection = meta.selection_width > 0;
        let diff = meta.kind == "main";
        let mask = meta.input.as_ref().map(|input| input.mask.clone()).unwrap_or_default();
        let fg = terminal_color(meta.fg).unwrap_or(INK);
        let name: SharedString = name.to_owned().into();

        range
            .filter_map(|i| {
                let line = view.lines.get(i)?;
                let selected = selection.is_some_and(|(first, last)| (first..=last).contains(&i));
                let background = if selected && !edge_selection {
                    Some(if active { SELECTION } else { SELECTION_INACTIVE })
                } else if diff && is_green(line.lead_fg) {
                    Some(ADDED_TINT)
                } else if diff && is_red(line.lead_fg) {
                    Some(REMOVED_TINT)
                } else {
                    None
                };
                let text: AnyElement = if mask.is_empty() {
                    StyledText::new(line.text.clone())
                        .with_highlights(line.highlights.iter().cloned())
                        .into_any_element()
                } else {
                    mask.repeat(line.text.chars().count()).into_any_element()
                };
                let view_name = name.clone();
                Some(
                    div()
                        .h(px(ROW))
                        .w_full()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_color(rgb(fg))
                        .when_some(background, |row, color| row.bg(rgb(color)))
                        .border_l_2()
                        .border_color(if selected && edge_selection {
                            rgb(SELECTION_EDGE)
                        } else {
                            rgba(0)
                        })
                        .pl_1()
                        .when(!meta.gutter_marker.is_empty(), |row| {
                            row.child(
                                div()
                                    .w(px(14.))
                                    .flex_none()
                                    .text_color(rgb(ACCENT))
                                    .when(meta.gutter.binary_search(&i).is_ok(), |gutter| {
                                        gutter.child(meta.gutter_marker.clone())
                                    }),
                            )
                        })
                        .child(text)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                this.click(&view_name, i, event)
                            }),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The content of a view as a scrollable list of its lines.
    fn list(&mut self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.model.view(name) else {
            return div().into_any_element();
        };
        let count = view.lines.len();
        let handle = self.scroll_handle(name);
        let id: SharedString = name.to_owned().into();
        let owned = name.to_owned();
        uniform_list(
            ElementId::Name(id),
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| this.rows(&owned, range, cx)),
        )
        .track_scroll(handle)
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn side_window(&mut self, window: &SideWindow, weight: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.model.view(&window.view) else {
            return div().into_any_element();
        };
        let meta = &view.meta;
        let focused = meta.focused;
        let (number, title) = split_jump_number(&meta.title);
        let footer = meta.footer.clone();
        let header_color = if focused { ACCENT } else { MUTED };

        let mut header = div()
            .flex_none()
            .h(px(28.))
            .px_2()
            .gap_1()
            .flex()
            .items_center()
            .text_color(rgb(header_color))
            .font_weight(FontWeight::SEMIBOLD)
            .when_some(number, |header, number| {
                header.child(div().text_color(rgb(MUTED)).child(number))
            });
        let active_tab = window.tab_views.iter().position(|tab| *tab == window.view);
        if window.tab_titles.is_empty() || active_tab.is_none() {
            let cursor = meta.cursor.max(0) as usize;
            let view_name = window.view.clone();
            header = header.child(
                div()
                    .id(ElementId::Name(format!("title-{}", window.name).into()))
                    .cursor_pointer()
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Outgoing::Click {
                            view: view_name.clone(),
                            x: 0,
                            y: cursor,
                            double: false,
                            extend: false,
                        })
                    })),
            );
        } else {
            for (index, tab) in window.tab_titles.iter().enumerate() {
                let active = Some(index) == active_tab;
                let window_name = window.name.clone();
                header = header.child(
                    div()
                        .id(ElementId::Name(format!("tab-{}-{index}", window.name).into()))
                        .px_1p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(active, |tab| tab.bg(rgb(PANEL)).text_color(rgb(header_color)))
                        .when(!active, |tab| tab.text_color(rgb(MUTED)).font_weight(FontWeight::NORMAL))
                        .child(tab.clone())
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(Outgoing::Tab {
                                window: window_name.clone(),
                                index,
                            })
                        })),
                );
            }
        }
        header = header.child(div().flex_1()).child(
            div()
                .text_color(rgb(MUTED))
                .font_weight(FontWeight::NORMAL)
                .child(footer),
        );

        div()
            .h(relative(weight))
            .min_h(px(28. + ROW))
            .flex()
            .flex_col()
            .overflow_hidden()
            .border_b_1()
            .border_color(rgb(RULE))
            .when(focused, |section| section.bg(rgb(PANEL)))
            .child(header)
            .child(self.list(&window.view.clone(), cx))
            .into_any_element()
    }

    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let windows = self.model.side_windows.clone();
        let weights: Vec<f32> = windows
            .iter()
            .map(|window| {
                let view = self.model.view(&window.view);
                match view {
                    _ if window.name == "status" => 0.6,
                    Some(view) if view.meta.focused => 3.,
                    _ => 1.,
                }
            })
            .collect();
        let total: f32 = weights.iter().sum::<f32>().max(1.);
        div()
            .w(relative(0.34))
            .max_w(px(460.))
            .min_w(px(220.))
            .flex_none()
            .flex()
            .flex_col()
            .bg(rgb(SIDEBAR))
            .border_r_1()
            .border_color(rgb(RULE))
            .children(
                windows
                    .iter()
                    .zip(weights)
                    .map(|(window, weight)| self.side_window(window, weight / total, cx))
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }

    fn pane(&mut self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.model.view(name) else {
            return div().into_any_element();
        };
        let meta = &view.meta;
        let (number, title) = split_jump_number(&meta.title);
        let subtitle = meta.subtitle.clone();
        let focused = meta.focused;
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
                    .h(px(28.))
                    .px_3()
                    .gap_1()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(RULE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(if focused { ACCENT } else { MUTED }))
                    .when_some(number, |header, number| {
                        header.child(div().text_color(rgb(MUTED)).child(number))
                    })
                    .child(title)
                    .child(div().flex_1())
                    .child(div().font_weight(FontWeight::NORMAL).child(subtitle)),
            )
            .child(self.list(name, cx))
            .into_any_element()
    }

    fn main_area(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let windows = self.model.main_windows.clone();
        let extras = self
            .model
            .view("extras")
            .is_some_and(|view| view.meta.window == "extras");
        let mut area = div().flex_1().min_w_0().flex().flex_col();
        for (index, window) in windows.iter().enumerate() {
            let pane = self.pane(&window.view, cx);
            area = area.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when(index > 0, |pane| pane.border_t_1().border_color(rgb(RULE)))
                    .child(pane),
            );
        }
        if extras {
            let log = self.pane("extras", cx);
            area = area.child(
                div()
                    .h(px(130.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .bg(rgb(SIDEBAR))
                    .border_t_1()
                    .border_color(rgb(RULE))
                    .child(log),
            );
        }
        area.into_any_element()
    }

    /// A single line of styled text from a view, e.g. the options bar.
    fn inline(&self, name: &str) -> Option<AnyElement> {
        let view = self.model.view(name)?;
        let fg = terminal_color(view.meta.fg).unwrap_or(INK);
        let children: Vec<AnyElement> = view
            .lines
            .iter()
            .filter(|line| !line.text.is_empty())
            .map(|line| {
                StyledText::new(line.text.clone())
                    .with_highlights(line.highlights.iter().cloned())
                    .into_any_element()
            })
            .collect();
        (!children.is_empty()).then(|| {
            div()
                .flex()
                .gap_3()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(fg))
                .children(children)
                .into_any_element()
        })
    }

    fn status_bar(&mut self) -> AnyElement {
        let mut bar = div()
            .flex_none()
            .h(px(30.))
            .px_3()
            .gap_3()
            .flex()
            .items_center()
            .border_t_1()
            .border_color(rgb(RULE))
            .text_color(rgb(MUTED))
            .overflow_hidden()
            .whitespace_nowrap();
        if self.model.view("search").is_some() {
            bar = bar
                .children(self.inline("searchPrefix"))
                .child(div().flex_1().child(self.input("search", false)));
        } else {
            bar = bar
                .children(self.inline("appStatus"))
                .child(div().flex_1().overflow_hidden().children(self.inline("options")))
                .children(self.inline("information"));
        }
        if !self.status.is_empty() {
            bar = bar.child(div().text_color(rgb(0xf07f7a)).child(self.status.clone()));
        }
        bar.into_any_element()
    }

    /// An editable view, drawn as a text field with a caret.
    fn input(&self, name: &str, multiline: bool) -> AnyElement {
        let Some(view) = self.model.view(name) else {
            return div().into_any_element();
        };
        let meta = &view.meta;
        let input = meta.input.clone().unwrap_or_default();
        let lines: Vec<String> = if view.lines.is_empty() {
            vec![String::new()]
        } else {
            view.lines.iter().map(|line| line.text.to_string()).collect()
        };
        let mask = |text: &str| {
            if input.mask.is_empty() {
                text.to_owned()
            } else {
                input.mask.repeat(text.chars().count())
            }
        };
        // Both fields of the commit message panel count as focused, but only
        // the one that keys go to has the caret.
        let has_caret = self.model.focus == name;
        let rows = lines.iter().enumerate().map(|(index, line)| {
            let mut row = div().h(px(ROW)).flex().items_center().whitespace_nowrap();
            if has_caret && index == input.cursor_line {
                let mut split = input.cursor_byte.min(line.len());
                while !line.is_char_boundary(split) {
                    split -= 1;
                }
                row = row
                    .child(mask(&line[..split]))
                    .child(div().w(px(2.)).h(px(15.)).bg(rgb(ACCENT)))
                    .child(mask(&line[split..]));
            } else {
                row = row.child(mask(line));
            }
            row
        });
        div()
            .id(ElementId::Name(format!("input-{name}").into()))
            .w_full()
            .px_2()
            .py_1()
            .bg(rgb(INPUT))
            .text_color(rgb(INK))
            .rounded_sm()
            .border_1()
            .border_color(rgb(if has_caret { ACCENT } else { RULE }))
            .when(multiline, |field| field.min_h(px(ROW * 6. + 10.)).max_h(px(ROW * 14. + 10.)))
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    fn popup_view(&mut self, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.model.view(name) else {
            return div().into_any_element();
        };
        let meta = view.meta.clone();
        let count = view.lines.len();
        let heading = (!meta.title.is_empty() || !meta.subtitle.is_empty()).then(|| {
            div()
                .flex()
                .gap_2()
                .text_color(rgb(if meta.focused { ACCENT } else { MUTED }))
                .font_weight(FontWeight::SEMIBOLD)
                .child(meta.title.clone())
                .child(div().flex_1())
                .child(
                    div()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(rgb(MUTED))
                        .child(meta.subtitle.clone()),
                )
        });
        let body = if meta.editable {
            self.input(name, name == "commitDescription" || count > 1)
        } else if meta.highlight {
            let height = (count.clamp(1, 16) as f32) * ROW;
            div()
                .h(px(height))
                .flex()
                .flex_col()
                .child(self.list(name, cx))
                .into_any_element()
        } else {
            let fg = terminal_color(meta.fg).unwrap_or(INK);
            let lines: Vec<AnyElement> = view
                .lines
                .iter()
                .map(|line| {
                    div()
                        .min_h(px(ROW))
                        .child(
                            StyledText::new(line.text.clone())
                                .with_highlights(line.highlights.iter().cloned()),
                        )
                        .into_any_element()
                })
                .collect();
            div()
                .id(ElementId::Name(format!("text-{name}").into()))
                .max_h(px(ROW * 18.))
                .overflow_y_scroll()
                .text_color(rgb(fg))
                .children(lines)
                .into_any_element()
        };
        let footer = (!meta.footer.is_empty())
            .then(|| div().text_color(rgb(MUTED)).child(meta.footer.clone()));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(heading)
            .child(body)
            .children(footer)
            .into_any_element()
    }

    fn popups(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let names = self.model.popups.clone();
        if names.is_empty() {
            return None;
        }
        let views: Vec<AnyElement> = names.iter().map(|name| self.popup_view(name, cx)).collect();
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(BACKDROP << 8 | 0xb0))
                .occlude()
                .child(
                    div()
                        .id("popup")
                        .w(relative(0.62))
                        .min_w(px(420.))
                        .max_w(px(900.))
                        .max_h(relative(0.88))
                        .overflow_y_scroll()
                        .p_4()
                        .gap_3()
                        .flex()
                        .flex_col()
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(RULE))
                        .rounded_md()
                        .shadow_lg()
                        .children(views),
                )
                .into_any_element(),
        )
    }

    fn external_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let external = self.model.external.as_ref()?;
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(BACKDROP << 8 | 0xd0))
                .occlude()
                .child(
                    div()
                        .w(relative(0.6))
                        .p_4()
                        .gap_3()
                        .flex()
                        .flex_col()
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(ACCENT))
                        .rounded_md()
                        .child(
                            div()
                                .text_color(rgb(ACCENT))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Running in an external terminal"),
                        )
                        .child(external.command.clone())
                        .child(div().text_color(rgb(MUTED)).child(
                            "LazyGit continues when the command exits. Finish it in the terminal window.",
                        ))
                        .child(
                            div()
                                .id("stop-waiting")
                                .px_3()
                                .py_1()
                                .rounded_sm()
                                .border_1()
                                .border_color(rgb(RULE))
                                .cursor_pointer()
                                .child("Stop waiting")
                                .on_click(cx.listener(|this, _, _, _| {
                                    this.send(Outgoing::CancelExternal)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// Splits lazygit's "[2]Files"-style title into the jump key and the title.
fn split_jump_number(title: &str) -> (Option<SharedString>, SharedString) {
    if let Some(rest) = title.strip_prefix('[') {
        if let Some((number, title)) = rest.split_once(']') {
            if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) {
                return (Some(number.to_owned().into()), title.to_owned().into());
            }
        }
    }
    (None, title.to_owned().into())
}

impl Render for Workbench {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .h(px(40.))
            .border_b_1()
            .border_color(rgb(RULE))
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(ACCENT))
                    .child("LAZYGIT"),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(rgb(MUTED))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.model.repo.clone()),
            )
            .child(div().text_color(rgb(MUTED)).child(self.model.context.clone()))
            .when_some(self.model.notice.clone(), |header, notice| {
                header.child(div().text_color(rgb(0xf07f7a)).child(notice))
            });

        let body = if self.model.received_frame {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.sidebar(cx))
                .child(self.main_area(cx))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("Starting LazyGit…")
                .into_any_element()
        };
        let status_bar = self.status_bar();
        let popups = self.popups(cx);
        let external = self.external_banner(cx);

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PAPER))
            .text_color(rgb(INK))
            .font_family("Menlo")
            .text_size(px(12.5))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(header)
            .child(body)
            .child(status_bar)
            .children(popups)
            .children(external)
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
    let (backend, mut events) = backend::launch(backend, &repo)?;
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1240.), px(800.)), cx);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.), px(400.))),
                ..Default::default()
            },
            move |window, cx| {
                let focus = cx.focus_handle();
                focus.focus(window);
                cx.new(|cx: &mut Context<Workbench>| {
                    cx.spawn(async move |this, cx| {
                        while let Some(event) = events.next().await {
                            if this.update(cx, |app, cx| app.handle(event, cx)).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    let activation = cx.observe_window_activation(window, |this, window, _| {
                        this.send(Outgoing::Focus {
                            focused: window.is_window_active(),
                        })
                    });
                    Workbench {
                        focus,
                        backend,
                        model: Model::default(),
                        status: SharedString::default(),
                        scroll_handles: HashMap::new(),
                        requested_lines: HashMap::new(),
                        _activation: activation,
                    }
                })
            },
        )
        .expect("open GPUI window");
        cx.activate(true);
    });
    Ok(())
}

//! The Tab command menu — a transient overlay listing the app's commands so the
//! learner can reach 辨析 / 词源 / 星座 / 设置 / 撤销 / 帮助 without memorizing
//! letter shortcuts. The primary "software化" surface: ↑↓ to move, Enter to fire,
//! and letters (a/w/g/s/?) still work as expert-mode direct triggers.
//!
//! Overlay priority sits below settings/ask/graph (those swallow input first) but
//! above help and the base card. Closing the menu is Tab or Esc; opening it is Tab.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState};

use super::app::{App, Stage};
use super::theme::*;

/// The runtime menu state: open flag + cursor index.
#[derive(Default)]
pub struct CommandMenu {
    pub open: bool,
    pub cursor: usize,
}

/// One selectable row in the command menu.
pub struct CommandItem {
    pub label: &'static str,
    pub hint: &'static str,
    pub shortcut: &'static str,
    /// When false the row is drawn dim and Enter on it is a no-op (e.g. "撤销评分"
    /// only makes sense inside the post-grade undo window).
    pub enabled: bool,
}

impl CommandMenu {
    /// Build the live command list for the current app state. Enabled mirrors the
    /// base key gates exactly — a row must not sit bright when its command would
    /// no-op (done state has no word to 辨析), and must not offer a spoiler the
    /// direct key deliberately withholds (词源/星座 stay dark until the reveal, so
    /// the recall isn't answered by a menu detour).
    pub fn items(&self, app: &App) -> Vec<CommandItem> {
        let undo_enabled = app.can_undo();
        let (is_new_prompt, revealed) = app
            .current
            .as_ref()
            .map(|c| {
                (
                    c.is_new && c.stage == Stage::Prompt,
                    c.stage == Stage::Revealed,
                )
            })
            .unwrap_or((false, false));
        vec![
            CommandItem {
                label: "对话",
                hint: if is_new_prompt {
                    "和 AI 一起推导这个词"
                } else if app.selected_example().is_some() {
                    "和 AI 解析选中的这个例句"
                } else if revealed {
                    "和 AI 分清易混词（多轮对话）"
                } else {
                    "揭示后可用"
                },
                shortcut: "a",
                enabled: is_new_prompt || revealed,
            },
            CommandItem {
                label: "语法速查",
                hint: "词性缩写与句子骨架的大白话说明（离线）",
                shortcut: "x",
                enabled: true,
            },
            CommandItem {
                label: "词源",
                hint: if revealed {
                    "打开 Wiktionary 词源页"
                } else {
                    "揭示后可用（先自己推）"
                },
                shortcut: "w",
                enabled: revealed,
            },
            CommandItem {
                label: "星座",
                hint: if revealed {
                    "当前词的词根家族"
                } else {
                    "揭示后可用（先自己推）"
                },
                shortcut: "g",
                enabled: revealed,
            },
            CommandItem {
                label: "设置",
                hint: "TTS 引擎切换（Kokoro/Matcha/Piper）",
                shortcut: "s",
                enabled: true,
            },
            CommandItem {
                label: "撤销评分",
                hint: if undo_enabled {
                    "撤销上一次评分（3s 内）"
                } else {
                    "评分后 3s 内可用"
                },
                shortcut: "u",
                enabled: undo_enabled,
            },
            CommandItem {
                label: "帮助",
                hint: "键位参考",
                shortcut: "?",
                enabled: true,
            },
        ]
    }

    /// Move cursor by `delta`, skipping disabled items so the cursor never lands
    /// on a row Enter can't fire. Wraparound matches the existing graph/ask nav.
    pub fn move_cursor(&mut self, delta: i32, items: &[CommandItem]) {
        if items.is_empty() {
            return;
        }
        let n = items.len() as i32;
        let mut next = self.cursor as i32;
        for _ in 0..n {
            next = (next + delta).rem_euclid(n);
            if items[next as usize].enabled {
                self.cursor = next as usize;
                return;
            }
        }
    }
}

/// Render the command menu as a centered popup. Disabled items are dimmed; the
/// focused row is reversed + bold. A footer line shows the navigation keys.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if !app.cmdmenu.open {
        return;
    }
    let items = app.cmdmenu.items(app);
    let list_items: Vec<ListItem> = items
        .iter()
        .map(|it| {
            let style = if it.enabled {
                Style::default().fg(FOAM)
            } else {
                Style::default().fg(MUTED)
            };
            let line = Line::from(vec![
                Span::styled(format!(" {:<6} ", it.label), style),
                Span::styled(it.hint.to_string(), Style::default().fg(MUTED)),
                Span::styled(
                    format!("   {:>1}", it.shortcut),
                    Style::default().fg(if it.enabled { CURRENT } else { MUTED }),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let popup = centered_rect(48, 56, area);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" 命令 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM));

    let mut state = ListState::default();
    state.select(Some(app.cmdmenu.cursor.min(items.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(list_items)
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(CURRENT)
                    .fg(SLATE)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ "),
        popup,
        &mut state,
    );

    // Footer — always visible so the learner knows the navigation contract.
    let footer = Line::from(Span::styled(
        " ↑↓ 选择 · Enter 确认 · 字母直达 · Esc/Tab 关闭 ",
        Style::default().fg(MUTED),
    ))
    .alignment(Alignment::Center);
    let footer_area = Rect {
        y: popup.bottom(),
        height: 1,
        ..popup
    };
    frame.render_widget(footer, footer_area);
}

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let [h] = Layout::horizontal([Constraint::Percentage(pct_x)])
        .flex(Flex::Center)
        .areas(area);
    let [v] = Layout::vertical([Constraint::Percentage(pct_y)])
        .flex(Flex::Center)
        .areas(h);
    v
}

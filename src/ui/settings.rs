//! The runtime settings overlay — switch TTS engine mid-session. `s` opens it when a
//! card is revealed (or the session is done). Lists the three engines with download
//! status; selecting an already-downloaded engine writes config, reloads TtsConfig,
//! and drops the warm session so the next synth uses the new engine.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::app::App;
use super::theme::*;
use crate::audio::tts::{from_kind, TtsEngineKind};
use crate::paths;

pub struct Settings {
    pub open: bool,
    pub cursor: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { open: false, cursor: 0 }
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if !app.settings.open {
        return;
    }

    let kinds = TtsEngineKind::all();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "↑↓ 选择 · Enter 切换 · s / Esc 关闭",
        Style::default().fg(MUTED),
    )));
    lines.push(Line::raw(""));

    for (i, kind) in kinds.iter().enumerate() {
        let eng = from_kind(*kind);
        let present = eng.models_present(&paths::engine_dir(*kind));
        let is_current = *kind == app.tts.kind;
        let is_cursor = i == app.settings.cursor;

        let mark = if is_cursor { "▸ " } else { "  " };
        let status = if present {
            Span::styled("✓ 已下载", Style::default().fg(GREEN))
        } else {
            Span::styled("✗ 未下载", Style::default().fg(CORAL))
        };
        let tag = if is_current {
            Span::styled("  · 当前", Style::default().fg(CURRENT))
        } else {
            Span::raw("")
        };

        let row_style = if is_cursor {
            Style::default().fg(FOAM).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FOAM_DIM)
        };

        lines.push(Line::from(vec![
            Span::styled(mark.to_string(), Style::default().fg(CURRENT)),
            Span::styled(
                format!("{:<8}", kind.id()),
                row_style,
            ),
            Span::styled(" · ", Style::default().fg(MUTED)),
            Span::styled(eng.blurb().to_string(), Style::default().fg(FOAM_DIM)),
            Span::styled("  ", Style::default().fg(MUTED)),
            status,
            tag,
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "未下载的引擎请退出后重跑 tuna 触发安装向导获取。",
        Style::default().fg(MUTED),
    )));

    let popup = centered_rect(80, 50, area);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" 设置 · 发音引擎 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: false }),
        popup,
    );
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

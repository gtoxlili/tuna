//! Rendering. The study screen: a thin status bar (gate + progress), the card,
//! and a context keybar. English/morphemes read as the machine's derivation voice;
//! the ZH meaning you arrive at glows amber.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, Stage};
use super::theme::*;
use crate::data::deck::parse_exchange;

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1), // status bar
        Constraint::Min(0),    // card
        Constraint::Length(1), // keybar
    ])
    .split(area);

    render_status(frame, chunks[0], app);
    if app.done() {
        render_done(frame, chunks[1], app);
    } else {
        render_card(frame, chunks[1], app);
    }
    render_keybar(frame, chunks[2], app);
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let gate = if app.gate.open {
        Span::styled(
            format!("● ROUTED · {}", app.gate.device.as_deref().unwrap_or("")),
            Style::default().fg(CURRENT),
        )
    } else {
        Span::styled(
            format!("○ 静默 · 未连接「{}」", app.needle),
            Style::default().fg(MUTED),
        )
    };
    let progress = Line::from(vec![
        Span::styled("拆联 ", Style::default().fg(MUTED)),
        Span::styled(app.session_new.to_string(), Style::default().fg(CURRENT)),
        Span::styled("  复习 ", Style::default().fg(MUTED)),
        Span::styled(app.session_reviews.to_string(), Style::default().fg(CURRENT)),
        Span::styled("  剩 ", Style::default().fg(MUTED)),
        Span::styled(app.remaining().to_string(), Style::default().fg(FOAM_DIM)),
    ])
    .alignment(Alignment::Right);

    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(gate)).style(Style::default().bg(SLATE)),
        cols[0],
    );
    frame.render_widget(progress.style(Style::default().bg(SLATE)), cols[1]);
}

fn render_card(frame: &mut Frame, area: Rect, app: &App) {
    let Some(c) = &app.current else { return };
    let mut lines: Vec<Line> = Vec::new();

    // phase eyebrow
    let (label, color) = if c.is_new {
        ("新词 · 拆联", CURRENT)
    } else {
        ("复习 · 提取", AMBER)
    };
    lines.push(Line::from(Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));

    // headword + IPA + pos
    let mut head = vec![Span::styled(
        &c.entry.word,
        Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
    )];
    if !c.entry.phonetic.is_empty() {
        head.push(Span::styled(
            format!("   /{}/", c.entry.phonetic),
            Style::default().fg(MUTED),
        ));
    }
    lines.push(Line::from(head));
    lines.push(Line::raw(""));

    match c.stage {
        Stage::Prompt => {
            let prompt = if c.is_new {
                "新词——先在脑中猜/拆一下它的意思，再回车揭示"
            } else {
                "回忆它的意思，再回车揭示"
            };
            lines.push(Line::from(vec![
                Span::styled("▸ ", Style::default().fg(CURRENT)),
                Span::styled(prompt, Style::default().fg(FOAM_DIM)),
            ]));
        }
        Stage::Revealed => {
            // meaning (amber) — the thing you arrive at
            for (i, t) in c.entry.translation.lines().take(4).enumerate() {
                let t = t.trim();
                if t.is_empty() {
                    continue;
                }
                let style = if i == 0 {
                    Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(AMBER)
                };
                lines.push(Line::from(Span::styled(t.to_string(), style)));
            }
            // EN definition
            if !c.entry.definition.is_empty() {
                lines.push(Line::raw(""));
                for d in c.entry.definition.lines().take(3) {
                    let d = d.trim();
                    if !d.is_empty() {
                        lines.push(Line::from(Span::styled(
                            d.to_string(),
                            Style::default().fg(FOAM_DIM),
                        )));
                    }
                }
            }
            // word-family (real graph data from ECDICT exchange) — Phase A only
            if c.is_new {
                let fam = parse_exchange(&c.entry.exchange);
                if !fam.is_empty() {
                    lines.push(Line::raw(""));
                    let mut spans = vec![Span::styled("同族  ", Style::default().fg(MUTED))];
                    for (i, (lab, form)) in fam.iter().enumerate() {
                        if i > 0 {
                            spans.push(Span::styled(" · ", Style::default().fg(MUTED)));
                        }
                        spans.push(Span::styled(form.clone(), Style::default().fg(CURRENT)));
                        spans.push(Span::styled(
                            format!("({lab})"),
                            Style::default().fg(MUTED),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
                if !c.entry.tag.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("考纲  ", Style::default().fg(MUTED)),
                        Span::styled(
                            c.entry.tag.replace(' ', " · "),
                            Style::default().fg(FOAM_DIM),
                        ),
                    ]));
                }
            }
        }
    }

    let block = Block::new()
        .style(Style::default().bg(ABYSS))
        .padding(Padding::new(4, 4, 2, 1));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_done(frame: &mut Frame, area: Rect, app: &App) {
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "✓ 本次会话完成",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("新学 ", Style::default().fg(MUTED)),
            Span::styled(app.session_new.to_string(), Style::default().fg(CURRENT)),
            Span::styled("  ·  复习 ", Style::default().fg(MUTED)),
            Span::styled(
                app.session_reviews.to_string(),
                Style::default().fg(CURRENT),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "明天词根会自己长出来。q 退出。",
            Style::default().fg(FOAM_DIM),
        )),
    ];
    let block = Block::new()
        .style(Style::default().bg(ABYSS))
        .padding(Padding::new(4, 4, 2, 1));
    frame.render_widget(
        Paragraph::new(lines).block(block).alignment(Alignment::Center),
        area,
    );
}

fn render_keybar(frame: &mut Frame, area: Rect, app: &App) {
    let spans = if app.done() {
        vec![key("q", "退出", CORAL)]
    } else {
        match app.current.as_ref().map(|c| c.stage) {
            Some(Stage::Prompt) => vec![
                key("Enter", "揭示", CURRENT),
                sep(),
                key("q", "退出", MUTED),
            ],
            _ => {
                let hints = app.interval_hints().unwrap_or_default();
                vec![
                    grade_key("1", "Again", &hints[0], CORAL),
                    grade_key("2", "Hard", &hints[1], AMBER),
                    grade_key("3", "Good", &hints[2], GREEN),
                    grade_key("4", "Easy", &hints[3], CURRENT),
                    sep(),
                    key("q", "退出", MUTED),
                ]
            }
        }
    };
    let mut flat = Vec::new();
    for group in spans {
        flat.extend(group);
    }
    frame.render_widget(
        Paragraph::new(Line::from(flat)).style(Style::default().bg(SLATE)),
        area,
    );
}

fn key(k: &str, label: &str, color: ratatui::style::Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!(" {k} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{label} "), Style::default().fg(MUTED)),
    ]
}

fn grade_key(k: &str, label: &str, interval: &str, color: ratatui::style::Color) -> Vec<Span<'static>> {
    vec![
        Span::styled(format!(" {k} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(label.to_string(), Style::default().fg(FOAM_DIM)),
        Span::styled(format!(" {interval}  "), Style::default().fg(MUTED)),
    ]
}

fn sep() -> Vec<Span<'static>> {
    vec![Span::styled("·  ", Style::default().fg(MUTED))]
}

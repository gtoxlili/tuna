//! Rendering. The study screen: a thin status bar (gate + progress), the card,
//! and a context keybar. English/morphemes read as the machine's derivation voice;
//! the ZH meaning you arrive at glows amber.

use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, Ask, CardView, Stage};
use super::theme::*;
use crate::data::deck::parse_exchange;
use crate::llm::enrich::Enrichment;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The current spinner glyph, advanced off the animation clock (~11 fps).
fn spin(app: &App) -> &'static str {
    SPINNER[(app.anim.elapsed().as_millis() / 90) as usize % SPINNER.len()]
}

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
    render_ask(frame, area, app);
}

/// The Socratic 辨析 popup, drawn over everything when active. The answer is
/// markdown (DeepSeek emits **bold** and `-` lists), so we parse it to styled
/// ratatui text rather than printing the raw syntax.
fn render_ask(frame: &mut Frame, area: Rect, app: &App) {
    let plain = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(FOAM)));
    let (title, color, mut lines) = match &app.ask {
        Ask::Idle => return,
        Ask::Pending => (
            "苏格拉底",
            MUTED,
            vec![Line::from(vec![
                Span::styled(format!("{} ", spin(app)), Style::default().fg(CURRENT)),
                Span::styled(
                    "让 DeepSeek 帮你把它和易混词的分别想清楚……",
                    Style::default().fg(FOAM_DIM),
                ),
            ])],
        ),
        Ask::Answer(t) => ("苏格拉底 · 辨析", CURRENT, tui_markdown::from_str(t).lines),
        Ask::Failed(e) => ("辨析失败", CORAL, vec![plain(e)]),
    };
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "a / Esc 关闭",
        Style::default().fg(MUTED),
    )));

    let popup = centered_rect(72, 72, area);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color))
        .title(format!(" {title} "))
        .title_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        // Base style so unstyled markdown text is readable; bold/headings layer on top.
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

    let enriched_new = c.is_new && c.enrichment.is_some();
    let eyebrow = match (c.is_new, c.stage, enriched_new) {
        (true, Stage::Prompt, true) => ("新词 · 拆", CURRENT),
        (true, Stage::Revealed, true) => ("新词 · 联", AMBER),
        (true, _, false) => ("新词 · 拆联", CURRENT),
        (false, _, _) => ("复习 · 提取", AMBER),
    };
    lines.push(Line::from(Span::styled(
        eyebrow.0,
        Style::default().fg(eyebrow.1).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));

    // headword + IPA
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
    // Frequency chip (from enrichment) — triage effort at a glance.
    if let Some(en) = &c.enrichment {
        if !en.freq_tier.is_empty() {
            let tc = match en.freq_tier.as_str() {
                "高频" => CORAL,
                "中频" => AMBER,
                _ => MUTED,
            };
            head.push(Span::styled(
                format!("    {}", en.freq_tier),
                Style::default().fg(tc),
            ));
        }
    }
    lines.push(Line::from(head));
    lines.push(Line::raw(""));

    match (c.stage, &c.enrichment) {
        // Phase A, enriched — the derivation experience
        (Stage::Prompt, Some(en)) if c.is_new => morpheme_prompt(&mut lines, en),
        (Stage::Revealed, Some(en)) if c.is_new => derivation_reveal(&mut lines, en),
        // Prompt (review, or un-enriched new)
        (Stage::Prompt, _) => {
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
        // Revealed (review, or un-enriched new) — plain meaning + ECDICT family
        (Stage::Revealed, _) => plain_meaning(&mut lines, c),
    }

    // Derive game (Phase A): the typed guess, echoed back on reveal to compare.
    if c.is_new {
        match c.stage {
            Stage::Prompt => {
                lines.push(Line::raw(""));
                let mut spans = vec![Span::styled("你的推测  ", Style::default().fg(MUTED))];
                if app.input.is_empty() {
                    spans.push(Span::styled(
                        "打下你推出的意思，Enter 揭示…",
                        Style::default().fg(MUTED),
                    ));
                } else {
                    spans.push(Span::styled(app.input.clone(), Style::default().fg(FOAM)));
                }
                spans.push(Span::styled("▋", Style::default().fg(CURRENT)));
                lines.push(Line::from(spans));
            }
            Stage::Revealed if !app.input.is_empty() => {
                lines.push(Line::raw(""));
                lines.push(Line::from(vec![
                    Span::styled("你刚推  ", Style::default().fg(MUTED)),
                    Span::styled(app.input.clone(), Style::default().fg(FOAM_DIM)),
                ]));
            }
            _ => {}
        }
    }

    // The graph made tangible: deck words you've already learned, same root.
    if c.is_new && !c.siblings.is_empty() {
        lines.push(Line::raw(""));
        let mut spans = vec![Span::styled("你学过  ", Style::default().fg(MUTED))];
        for (i, (w, _)) in c.siblings.iter().take(5).enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ", Style::default().fg(MUTED)));
            }
            spans.push(Span::styled(w.clone(), Style::default().fg(GREEN)));
        }
        if let Some((_, via)) = c.siblings.first() {
            spans.push(Span::styled(
                format!("   同根 {via}"),
                Style::default().fg(MUTED),
            ));
        }
        lines.push(Line::from(spans));
    }

    let block = Block::new()
        .style(Style::default().bg(ABYSS))
        .padding(Padding::new(4, 4, 2, 1));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// The morpheme cells. Ownership is no longer a baked flag on the morpheme — it
/// surfaces through the live "你学过" siblings line (P2 will color cells by real mastery).
fn morpheme_cells(en: &Enrichment) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, m) in en.morphemes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().fg(MUTED)));
        }
        let color = CURRENT;
        spans.push(Span::styled("⟦ ", Style::default().fg(MUTED)));
        spans.push(Span::styled(
            m.unit.clone(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if !m.meaning_zh.is_empty() {
            spans.push(Span::styled(
                format!(" {}", m.meaning_zh),
                Style::default().fg(FOAM_DIM),
            ));
        }
        spans.push(Span::styled(" ⟧", Style::default().fg(MUTED)));
    }
    spans
}

/// Phase A prompt: show the parts, hand over the anchors, ask them to derive.
fn morpheme_prompt(lines: &mut Vec<Line>, en: &Enrichment) {
    if en.morphemes.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(CURRENT)),
            Span::styled(
                "先在脑中猜一下它的意思，再回车揭示",
                Style::default().fg(FOAM_DIM),
            ),
        ]));
    } else {
        let mut cells = morpheme_cells(en);
        cells.push(Span::styled("   →  ?", Style::default().fg(MUTED)));
        lines.push(Line::from(cells));
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(CURRENT)),
            Span::styled(
                "已知这些词素，先自己推出整词的意思，再回车揭示",
                Style::default().fg(FOAM_DIM),
            ),
        ]));
    }
}

/// Phase A reveal: the derivation chain, meaning, honest etymology, examples, confusables.
fn derivation_reveal(lines: &mut Vec<Line>, en: &Enrichment) {
    if !en.morphemes.is_empty() {
        lines.push(Line::from(morpheme_cells(en)));
        lines.push(Line::raw(""));
    }
    // derivation chain, final segment in amber
    if !en.derivation_zh.is_empty() {
        let parts: Vec<&str> = en
            .derivation_zh
            .split('→')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        let mut spans = vec![Span::styled("推导  ", Style::default().fg(MUTED))];
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" → ", Style::default().fg(MUTED)));
            }
            let last = i == parts.len() - 1;
            let style = if last {
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FOAM_DIM)
            };
            spans.push(Span::styled(p.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    // gloss
    if !en.gloss_zh.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("释义  ", Style::default().fg(MUTED)),
            Span::styled(
                en.gloss_zh.clone(),
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    // honest etymology badge (+ hook)
    let (badge, bcolor) = match en.etymology_confidence.as_str() {
        "solid" => ("● 真实词源", GREEN),
        "folk" => ("● 俗词源(助记)", AMBER),
        "mnemonic" => ("● 记忆钩子", MUTED),
        _ => ("", MUTED),
    };
    if !badge.is_empty() {
        let mut spans = vec![Span::styled(badge, Style::default().fg(bcolor))];
        if !en.hook.is_empty() {
            spans.push(Span::styled(
                format!("   {}", en.hook),
                Style::default().fg(MUTED),
            ));
        }
        lines.push(Line::from(spans));
    }
    // examples (CET-4 friendly first, then 考研)
    let mut shown = 0;
    for ex in &en.examples {
        if ex.en.is_empty() || shown >= 2 {
            continue;
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("例  ", Style::default().fg(MUTED)),
            Span::styled(ex.en.clone(), Style::default().fg(FOAM)),
        ]));
        if !ex.zh.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    {}", ex.zh),
                Style::default().fg(FOAM_DIM),
            )));
        }
        shown += 1;
    }
    // confusables (coral) — the 形近/近义 the exam loves
    let conf: Vec<_> = en
        .graph_edges
        .iter()
        .filter(|e| e.relation == "confusable")
        .take(2)
        .collect();
    if !conf.is_empty() {
        lines.push(Line::raw(""));
        for e in conf {
            let why = if e.why_zh.is_empty() {
                String::new()
            } else {
                format!(" — {}", e.why_zh)
            };
            lines.push(Line::from(vec![
                Span::styled("辨析  ", Style::default().fg(MUTED)),
                Span::styled(e.target.clone(), Style::default().fg(CORAL)),
                Span::styled(why, Style::default().fg(FOAM_DIM)),
            ]));
        }
    }
}

/// Plain meaning for review cards and un-enriched new words: ZH gloss + EN def +
/// ECDICT word-family.
fn plain_meaning(lines: &mut Vec<Line>, c: &CardView) {
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
    if !c.entry.definition.is_empty() {
        lines.push(Line::raw(""));
        for d in c.entry.definition.lines().take(2) {
            let d = d.trim();
            if !d.is_empty() {
                lines.push(Line::from(Span::styled(
                    d.to_string(),
                    Style::default().fg(FOAM_DIM),
                )));
            }
        }
    }
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
                spans.push(Span::styled(format!("({lab})"), Style::default().fg(MUTED)));
            }
            lines.push(Line::from(spans));
        }
    }
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
    let is_new_prompt = matches!(
        app.current.as_ref().map(|c| (c.is_new, c.stage)),
        Some((true, Stage::Prompt))
    );
    let spans = if app.done() {
        vec![key("q", "退出", CORAL)]
    } else if is_new_prompt {
        // Derive game: typed keys build your guess, so quitting is Esc.
        vec![
            key("Enter", "揭示", CURRENT),
            key("Space", "发音", MUTED),
            key("Esc", "退出", MUTED),
        ]
    } else {
        match app.current.as_ref().map(|c| c.stage) {
            Some(Stage::Prompt) => vec![
                key("Enter", "揭示", CURRENT),
                key("Space", "发音", MUTED),
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
                    key("Space", "发音", MUTED),
                    key("a", "辨析", MUTED),
                    key("q", "退出", MUTED),
                ]
            }
        }
    };
    let mut flat = Vec::new();
    for group in spans {
        flat.extend(group);
    }
    // Status on the left: synth spinner outranks the transient audio message.
    let status: Option<Span> = if let Some(w) = &app.tts_pending {
        Some(Span::styled(
            format!(" {} 合成中 {w}   ", spin(app)),
            Style::default().fg(CURRENT),
        ))
    } else {
        app.audio_msg
            .as_ref()
            .map(|msg| Span::styled(format!(" {msg}   "), Style::default().fg(CURRENT)))
    };
    if let Some(s) = status {
        let mut with = vec![s];
        with.extend(flat);
        flat = with;
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

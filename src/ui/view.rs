//! Rendering. The study screen: a thin status bar (gate + progress), the card,
//! and a context keybar. English/morphemes read as the machine's derivation voice;
//! the ZH meaning you arrive at glows amber.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

use super::app::{
    App, Ask, CardView, DeriveState, MORPHEME_CELL_FADE_MS, MORPHEME_STAGGER_MS, Stage, Strike,
};
use super::settings;
use super::theme::*;
use crate::data::deck::parse_exchange;
use crate::llm::enrich::Enrichment;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Linear blend of two RGB colors. `t=0` → `a`, `t=1` → `b`. Used for fade-in math
/// (card slide and morpheme stagger). Non-RGB colors return `b` as a no-op fallback.
fn blend(a: Color, b: Color, t: f64) -> Color {
    let (ar, ag, ab) = match a {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return b,
    };
    let (br, bg, bb) = match b {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return b,
    };
    let lerp = |x: u8, y: u8| -> u8 { (x as f64 * (1.0 - t) + y as f64 * t).round() as u8 };
    Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
}

/// The current spinner glyph, advanced off the animation clock (~11 fps).
/// Reduced-motion users get a static glyph — the spinner is the most persistent
/// animation in the app (LLM/TTS runs 5-30s) and motion sensitivity matters here.
fn spin(app: &App) -> &'static str {
    if app.reduced_motion {
        "○"
    } else {
        SPINNER[(app.anim.elapsed().as_millis() / 90) as usize % SPINNER.len()]
    }
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
    render_derive_chat(frame, area, app);
    render_constellation(frame, area, app);
    settings::render(frame, area, app);
    super::cmdmenu::render(frame, area, app);
    render_help(frame, area, app);
}

/// The constellation overlay: the current word's root-family, drawn as morpheme hubs
/// with the deck words orbiting each. Every edge is a real shared node; a word glows
/// only if you've actually learned it (green = FSRS-solid, amber = still fresh); the
/// dim ones are the frontier — each is one root away from being derivable, not rote.
fn render_constellation(frame: &mut Frame, area: Rect, app: &App) {
    if !app.show_graph {
        return;
    }
    let word = app
        .current
        .as_ref()
        .map(|c| c.entry.word.as_str())
        .unwrap_or("");

    // Solid ≥21d of memory stability, fresh below; the current word is teal.
    let glow = |m: &crate::data::deck::GraphMember| -> (Color, &'static str) {
        if m.word == word {
            (CURRENT, "◉")
        } else if !m.introduced {
            (MUTED, "·")
        } else if m.stability >= 21.0 {
            (GREEN, "✦")
        } else {
            (AMBER, "✦")
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut lit_total = 0usize;
    let mut orbit_total = 0usize;
    let mut flat_i = 0usize;
    for g in &app.graph {
        // Order the orbit: current word first, then what's lit (steadiest first),
        // then the dim frontier — capped so a noisy suffix like -tion stays readable.
        // Shared sort+cap with `App::graph_members` so the cursor index lines up.
        let mut ms = g.members.clone();
        crate::data::deck::sort_members(&mut ms, word);
        let lit = ms.iter().filter(|m| m.introduced).count();
        lit_total += lit;
        orbit_total += ms.len();

        let gloss = if g.gloss_zh.is_empty() {
            String::new()
        } else {
            format!("  {}", g.gloss_zh)
        };
        lines.push(Line::from(vec![
            Span::styled("词根 ", Style::default().fg(MUTED)),
            Span::styled(
                g.surface.clone(),
                Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(gloss, Style::default().fg(FOAM_DIM)),
            Span::styled(
                format!("   ✦ 点亮 {}/{}", lit, ms.len()),
                Style::default().fg(MUTED),
            ),
        ]));

        const CAP: usize = crate::data::deck::GRAPH_MEMBER_CAP;
        let hidden = ms.len().saturating_sub(CAP);
        let mut chips: Vec<Span> = Vec::new();
        for m in ms.iter().take(CAP) {
            let is_cursor = flat_i == app.graph_cursor;
            let (color, mark) = if is_cursor { (CURRENT, "▸") } else { glow(m) };
            let style = if m.word == word || is_cursor {
                let mut s = Style::default().fg(color).add_modifier(Modifier::BOLD);
                if is_cursor {
                    s = s.add_modifier(Modifier::REVERSED);
                }
                s
            } else {
                Style::default().fg(color)
            };
            chips.push(Span::styled(format!("{mark} {}   ", m.word), style));
            flat_i += 1;
        }
        if hidden > 0 {
            chips.push(Span::styled(
                format!("+{hidden} …"),
                Style::default().fg(MUTED),
            ));
        }
        lines.push(Line::from(chips));
        lines.push(Line::raw(""));
    }

    // Header + a one-line honest summary of what's real here.
    let mut head = vec![
        Line::from(vec![
            Span::styled("✦ 星座 · ", Style::default().fg(CURRENT)),
            Span::styled(
                word.to_string(),
                Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            format!("你的星系里，这些根上已点亮 {lit_total}/{orbit_total} 颗"),
            Style::default().fg(FOAM_DIM),
        )),
        Line::raw(""),
    ];
    head.append(&mut lines);
    head.push(Line::from(vec![
        Span::styled("◉ 当前   ", Style::default().fg(CURRENT)),
        Span::styled("✦ 已点亮", Style::default().fg(GREEN)),
        Span::styled("(越绿越稳固)   ", Style::default().fg(MUTED)),
        Span::styled("· 待解锁", Style::default().fg(MUTED)),
        Span::styled("(只差这个词根)", Style::default().fg(MUTED)),
    ]));
    head.push(Line::from(Span::styled(
        "↑↓ 导航 · Space 朗读 · g / Esc 关闭",
        Style::default().fg(MUTED),
    )));

    let popup = centered_rect(78, 82, area);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" 星座 · 词根家族 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(
        Paragraph::new(head).block(block).wrap(Wrap { trim: false }),
        popup,
    );
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
    // ↑↓ only scrolls an Answer — advertising it during Pending would be a lie.
    let hint = if matches!(&app.ask, Ask::Answer(_)) {
        "a / Esc 关闭  ·  ↑↓ 滚动"
    } else {
        "a / Esc 关闭"
    };
    lines.push(Line::from(Span::styled(hint, Style::default().fg(MUTED))));

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
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.ask_scroll() as u16, 0)),
        popup,
    );
}

/// The derivation chat popup (Phase A "拆"): a multi-turn conversation with the LLM
/// where the learner guesses the word's meaning from its morphemes and gets Socratic
/// guidance. Shows the conversation history (scrollable) + an input line at the bottom.
fn render_derive_chat(frame: &mut Frame, area: Rect, app: &App) {
    if app.derive == DeriveState::Closed {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();

    // Conversation history — each turn labeled by role.
    for turn in &app.derive_turns {
        if turn.is_user {
            lines.push(Line::from(vec![
                Span::styled("你  ", Style::default().fg(CURRENT)),
                Span::styled(turn.text.clone(), Style::default().fg(FOAM)),
            ]));
        } else {
            let md = tui_markdown::from_str(&turn.text);
            lines.push(Line::from(vec![Span::styled(
                "✦  ",
                Style::default().fg(AMBER),
            )]));
            lines.extend(md.lines);
        }
        lines.push(Line::raw(""));
    }

    // Input line or pending spinner.
    if app.derive == DeriveState::Pending {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", spin(app)), Style::default().fg(CURRENT)),
            Span::styled("思考中……", Style::default().fg(FOAM_DIM)),
        ]));
    } else {
        let input_span = if app.input.is_empty() {
            Span::styled("说说你看到了哪些词素……", Style::default().fg(MUTED))
        } else {
            Span::styled(app.input.clone(), Style::default().fg(FOAM))
        };
        lines.push(Line::from(vec![
            Span::styled("▸  ", Style::default().fg(CURRENT)),
            input_span,
            Span::styled("▋", Style::default().fg(CURRENT)),
        ]));
    }

    // Footer — honest per state: while Pending there is nothing to send.
    lines.push(Line::raw(""));
    let footer = if app.derive == DeriveState::Pending {
        "Esc 收起（回复就绪会提示） · ↑↓ 滚动"
    } else {
        "Enter 发送 · Esc 收起（对话保留） · ↑↓ 滚动"
    };
    lines.push(Line::from(Span::styled(
        footer,
        Style::default().fg(MUTED),
    )));

    let popup = centered_rect(72, 72, area);

    // Pin the view to the BOTTOM: the newest reply and the input line are the live
    // edge of a chat, so `derive_scroll` counts rows up from the bottom (0 = pinned)
    // and we convert it into a top offset here. Wrapped-row counts are estimated
    // (ASCII = 1 col, everything else = 2) — a small over-estimate only leaves a
    // blank line above, never hides the input.
    let inner_w = popup.width.saturating_sub(2 + 4).max(8) as usize; // borders + l/r padding
    let inner_h = popup.height.saturating_sub(2 + 2) as usize; // borders + t/b padding
    let row_count = |l: &Line| -> usize {
        let cols: usize = l
            .spans
            .iter()
            .flat_map(|s| s.content.chars())
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum();
        cols.max(1).div_ceil(inner_w)
    };
    let total_rows: usize = lines.iter().map(row_count).sum();
    let offset = total_rows
        .saturating_sub(inner_h)
        .saturating_sub(app.derive_scroll);

    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" 推导 · 和 AI 一起拆词 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((offset as u16, 0)),
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
    // Truncate long device names so a verbose Bluetooth name ("某人的 AirPods Pro
    // Max (2nd generation)") can't blow the 1-line status bar past the progress
    // column. Half the bar minus the "● ROUTED · " prefix (~14 cols) is the budget.
    let dev = app.gate.device.as_deref().unwrap_or("");
    let half = area.width.saturating_div(2) as usize;
    let budget = half.saturating_sub(14);
    let dev_short = if dev.chars().count() > budget {
        let truncated: String = dev.chars().take(budget.saturating_sub(1)).collect();
        format!("{truncated}…")
    } else {
        dev.to_string()
    };
    let gate = if app.gate.open {
        Span::styled(
            format!("● ROUTED · {dev_short}"),
            Style::default().fg(CURRENT),
        )
    } else {
        Span::styled(
            format!("○ 静默 · 未连接「{}」", app.needle),
            Style::default().fg(MUTED),
        )
    };
    // Add the card position so the learner can tell "今日已学完" (remaining 0) from
    // "本次完成" — the count alone didn't distinguish. Shown 1-based while a card is
    // up ("第 3 张" reads 3/15, not 2/15); done pins at total/total.
    let pos_label = if app.session_total > 0 {
        let cur = if app.done() {
            app.session_total
        } else {
            (app.pos + 1).min(app.session_total)
        };
        format!("{}/{}", cur, app.session_total)
    } else {
        String::new()
    };
    let progress = Line::from(vec![
        Span::styled("拆联 ", Style::default().fg(MUTED)),
        Span::styled(app.session_new.to_string(), Style::default().fg(CURRENT)),
        Span::styled("  复习 ", Style::default().fg(MUTED)),
        Span::styled(
            app.session_reviews.to_string(),
            Style::default().fg(CURRENT),
        ),
        Span::styled("  剩 ", Style::default().fg(MUTED)),
        Span::styled(app.remaining().to_string(), Style::default().fg(FOAM_DIM)),
        Span::styled(format!("  {pos_label}"), Style::default().fg(MUTED)),
    ])
    .alignment(Alignment::Right);

    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
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
    let speak_mark = if c.stage == Stage::Revealed && c.speak_cursor == 0 {
        Span::styled(
            "▸ ",
            Style::default()
                .fg(CURRENT)
                .add_modifier(Modifier::REVERSED),
        )
    } else {
        Span::raw("  ")
    };
    let mut head = vec![
        speak_mark,
        Span::styled(
            &c.entry.word,
            Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
        ),
    ];
    if !c.entry.phonetic.is_empty() {
        head.push(Span::styled(
            format!("   /{}/", c.entry.phonetic),
            Style::default().fg(MUTED),
        ));
    }
    // Frequency chip (from enrichment) — triage effort at a glance.
    if let Some(en) = &c.enrichment
        && !en.freq_tier.is_empty() {
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
    lines.push(Line::from(head));
    lines.push(Line::raw(""));

    // Morphemes you've already "earned" (encountered in a learned sibling) light
    // green; the rest stay teal/new. The sibling's `via` is the shared morpheme
    // surface — matching it against the enrichment `unit` shows which roots you
    // already own vs. which are new scaffolding on this card.
    let owned: Vec<String> = c.siblings.iter().map(|(_, via)| via.clone()).collect();

    match (c.stage, &c.enrichment) {
        // Phase A, enriched — the derivation experience
        (Stage::Prompt, Some(en)) if c.is_new => morpheme_prompt(&mut lines, en, &owned),
        (Stage::Revealed, Some(en)) if c.is_new => derivation_reveal(
            &mut lines,
            en,
            c.speak_cursor,
            app.reveal_elapsed_ms(),
            &owned,
        ),
        // Prompt (review, or un-enriched new)
        (Stage::Prompt, _) => {
            let prompt = if c.is_new {
                "新词：先在脑中猜/拆一下它的意思，再回车揭示"
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

    // Derive chat hint (Phase A Prompt): invite the learner to chat with the LLM
    // about the derivation. The chat itself opens via `a` (see render_derive_chat).
    // A collapsed-but-alive conversation changes the invitation to a resumption —
    // the learner should know their thread is still there.
    if c.is_new && matches!(c.stage, Stage::Prompt) {
        lines.push(Line::raw(""));
        if app.derive_turns.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("想拆词？按 ", Style::default().fg(MUTED)),
                Span::styled("a", Style::default().fg(CURRENT)),
                Span::styled(" 和 AI 一起推导", Style::default().fg(MUTED)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("按 ", Style::default().fg(MUTED)),
                Span::styled("a", Style::default().fg(CURRENT)),
                Span::styled(
                    format!(" 继续推导对话（已 {} 条）", app.derive_turns.len()),
                    Style::default().fg(MUTED),
                ),
            ]));
        }
    }

    // 星火接线 — after the reveal, EARN the edge by recalling a learned sibling.
    // (In Prompt we stay quiet so the siblings list never spoils the recall.)
    if c.is_new && c.stage == Stage::Revealed {
        match c.strike {
            Strike::Prompt => {
                if let Some(a) = &c.anchor {
                    lines.push(Line::raw(""));
                    lines.push(Line::from(vec![
                        Span::styled("✦ 词根 ", Style::default().fg(AMBER)),
                        Span::styled(
                            a.surface.clone(),
                            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("({}) ", a.gloss_zh), Style::default().fg(MUTED)),
                        Span::styled("你在哪个已学的词里见过？", Style::default().fg(FOAM)),
                    ]));
                    lines.push(Line::from(Span::styled(
                        "  在脑中想出那个词，Space 翻牌",
                        Style::default().fg(MUTED),
                    )));
                }
            }
            Strike::Flipped => {
                if let Some(a) = &c.anchor {
                    lines.push(Line::raw(""));
                    lines.push(Line::from(vec![
                        Span::styled("✦ 你学过  ", Style::default().fg(AMBER)),
                        Span::styled(
                            a.word.clone(),
                            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("  （{} · {}）", a.surface, a.gloss_zh),
                            Style::default().fg(MUTED),
                        ),
                    ]));
                    lines.push(Line::from(Span::styled(
                        "  想起来了吗？ y 记得 · n 想不起",
                        Style::default().fg(FOAM_DIM),
                    )));
                }
            }
            Strike::Idle => {
                // The earned strike arc, firing from the recalled anchor into the new word.
                // Siblings render ALWAYS — the arc overlays on top rather than replacing the
                // "you've learned" line, so non-reduced-motion users (who see the 400ms arc)
                // and reduced-motion users (instant siblings) read the same content. Was
                // `else if` — meaning during the arc the siblings vanished, a 900ms content gap.
                if !c.siblings.is_empty() {
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
                if let (Some(p), Some(a)) = (app.strike_progress(), &c.anchor) {
                    lines.push(Line::raw(""));
                    let total = 12usize;
                    let filled = ((p * total as f64).round() as usize).min(total);
                    let bar = format!("{}{}", "━".repeat(filled), "╌".repeat(total - filled));
                    let mut spans = vec![
                        Span::styled(
                            "✦ ",
                            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            a.word.clone(),
                            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!(" {bar}⟶ "), Style::default().fg(AMBER)),
                        Span::styled(
                            &c.entry.word,
                            Style::default().fg(CURRENT).add_modifier(Modifier::BOLD),
                        ),
                    ];
                    if p >= 0.9 {
                        spans.push(Span::styled(
                            "  ✦ 接入星座",
                            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                        ));
                    }
                    lines.push(Line::from(spans));
                }
            }
        }
    }

    // Post-grade flash: tint the card background briefly in the rating's color so the
    // act of grading has tactile weight (Again=red / Hard=amber / Good=green / Easy=cyan).
    let flash_bg = app.grade_flash().and_then(|(rating, p)| {
        // Fade out over the flash window: full strength first half, dim second half.
        let strength = if p < 0.5 { 0.18 } else { 0.09 };
        let base = match rating {
            rs_fsrs::Rating::Again => CORAL,
            rs_fsrs::Rating::Hard => AMBER,
            rs_fsrs::Rating::Good => GREEN,
            rs_fsrs::Rating::Easy => CURRENT,
        };
        let ratatui::style::Color::Rgb(r, g, b) = base else {
            return None;
        };
        Some(ratatui::style::Color::Rgb(
            ((r as f32 * strength) + ABYSS_R as f32 * (1.0 - strength)) as u8,
            ((g as f32 * strength) + ABYSS_G as f32 * (1.0 - strength)) as u8,
            ((b as f32 * strength) + ABYSS_B as f32 * (1.0 - strength)) as u8,
        ))
    });
    // Card slide-in: when a new card just loaded, fade its text up from the card bg
    // over 150ms so it "arrives" rather than jump-cutting. We blend every span's fg
    // toward ABYSS (the card bg) by (1-p) — at p=0 the text is invisible (fg=bg),
    // at p=1 it's full strength. Grade flash and slide-in don't overlap (grade →
    // load_current clears grade_flash and primes slide), so the two bg tints don't
    // fight. Reduced-motion skips this entirely (card_slide stays None).
    if let Some(p) = app.card_slide_progress() {
        let fade = 1.0 - p;
        for line in lines.iter_mut() {
            for span in line.spans.iter_mut() {
                let mut style = span.style;
                if let Some(Color::Rgb(r, g, b)) = style.fg {
                    let nr = (r as f64 * (1.0 - fade) + ABYSS_R as f64 * fade) as u8;
                    let ng = (g as f64 * (1.0 - fade) + ABYSS_G as f64 * fade) as u8;
                    let nb = (b as f64 * (1.0 - fade) + ABYSS_B as f64 * fade) as u8;
                    style = style.fg(Color::Rgb(nr, ng, nb));
                    span.style = style;
                }
            }
        }
    }

    let block = Block::new()
        .style(Style::default().bg(flash_bg.unwrap_or(ABYSS)))
        .padding(Padding::new(4, 4, 2, 1));
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

/// The morpheme cells. Ownership is no longer a baked flag on the morpheme — it
/// surfaces through the live "你学过" siblings line (P2 will color cells by real mastery).
fn morpheme_cells(
    en: &Enrichment,
    reveal_ms: Option<u128>,
    owned: &[String],
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, m) in en.morphemes.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", Style::default().fg(MUTED)));
        }
        // Per-cell stagger: cell i starts at i × 60ms and fades in over 120ms. Before
        // its window the cell is dimmed toward MUTED; once settled, full color. A
        // None reveal_ms (reduced-motion or post-window) renders all cells solid.
        let cell_p = reveal_ms.map(|ms| {
            let start = (i as u128) * MORPHEME_STAGGER_MS;
            let local = ms.saturating_sub(start);
            (local as f64 / MORPHEME_CELL_FADE_MS as f64).clamp(0.0, 1.0)
        });
        // D1: a morpheme you've already earned (seen in a learned sibling) reads
        // green ("you own this"); a new one reads teal ("new scaffolding"). The fade
        // below eases from MUTED up to the target color so the stagger still reads.
        let is_owned = owned.iter().any(|s| s.eq_ignore_ascii_case(&m.unit));
        let target_color = if is_owned { GREEN } else { CURRENT };
        let dim = |target: Color, p: f64| -> Color { blend(target, MUTED, 1.0 - p) };
        let unit_color = cell_p.map(|p| dim(target_color, p)).unwrap_or(target_color);
        let meaning_color = cell_p.map(|p| dim(FOAM_DIM, p)).unwrap_or(FOAM_DIM);
        spans.push(Span::styled("⟦ ", Style::default().fg(MUTED)));
        spans.push(Span::styled(
            m.unit.clone(),
            Style::default().fg(unit_color).add_modifier(Modifier::BOLD),
        ));
        if !m.meaning_zh.is_empty() {
            spans.push(Span::styled(
                format!(" {}", m.meaning_zh),
                Style::default().fg(meaning_color),
            ));
        }
        spans.push(Span::styled(" ⟧", Style::default().fg(MUTED)));
    }
    spans
}

/// Phase A prompt: show the parts, hand over the anchors, ask them to derive.
fn morpheme_prompt(lines: &mut Vec<Line>, en: &Enrichment, owned: &[String]) {
    if en.morphemes.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("▸ ", Style::default().fg(CURRENT)),
            Span::styled(
                "先在脑中猜一下它的意思，再回车揭示",
                Style::default().fg(FOAM_DIM),
            ),
        ]));
    } else {
        // Prompt shows the morphemes as "given" — no stagger (they're the scaffolding),
        // but owned coloring applies so you can see which roots you already bring.
        let mut cells = morpheme_cells(en, None, owned);
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
fn derivation_reveal(
    lines: &mut Vec<Line>,
    en: &Enrichment,
    speak_cursor: usize,
    reveal_ms: Option<u128>,
    owned: &[String],
) {
    if !en.morphemes.is_empty() {
        lines.push(Line::from(morpheme_cells(en, reveal_ms, owned)));
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
        let speak_mark = if speak_cursor == shown + 1 {
            Span::styled(
                "▸ ",
                Style::default()
                    .fg(CURRENT)
                    .add_modifier(Modifier::REVERSED),
            )
        } else {
            Span::raw("  ")
        };
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            speak_mark,
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
            "明天词根会自己长出来。Esc 退出。",
            Style::default().fg(FOAM_DIM),
        )),
    ];
    let block = Block::new()
        .style(Style::default().bg(ABYSS))
        .padding(Padding::new(4, 4, 2, 1));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        area,
    );
}

fn render_keybar(frame: &mut Frame, area: Rect, app: &App) {
    let is_new_prompt = matches!(
        app.current.as_ref().map(|c| (c.is_new, c.stage)),
        Some((true, Stage::Prompt))
    );
    let strike = app
        .current
        .as_ref()
        .map(|c| c.strike)
        .unwrap_or(Strike::Idle);
    let mut spans = if app.derive != DeriveState::Closed {
        // Chat input owns the keyboard — the base shortcuts underneath are dead,
        // so showing them would be a lie. Mirror the popup's footer.
        if app.derive == DeriveState::Pending {
            vec![key("↑↓", "滚动", MUTED), key("Esc", "收起", MUTED)]
        } else {
            vec![
                key("Enter", "发送", CURRENT),
                key("⌫", "删字", MUTED),
                key("↑↓", "滚动", MUTED),
                key("Esc", "收起", MUTED),
            ]
        }
    } else if app.done() {
        // Settings/Tab stay live after the session — surface them.
        vec![
            key("s", "设置", MUTED),
            key("Tab", "命令", MUTED),
            key("Esc", "退出", CORAL),
        ]
    } else if strike == Strike::Prompt {
        vec![key("Space", "翻牌", AMBER), key("Esc", "跳过", MUTED)]
    } else if strike == Strike::Flipped {
        vec![
            key("y", "记得", GREEN),
            key("n", "想不起", CORAL),
            key("Esc", "跳过", MUTED),
        ]
    } else if is_new_prompt {
        // New word Prompt: shortcuts are live (no input capture). `a` opens the
        // derivation chat; Enter reveals the full derivation chain.
        vec![
            key("Enter", "揭示", CURRENT),
            key("a", "推导", AMBER),
            key("Space", "发音", MUTED),
            key("Tab", "命令", MUTED),
            key("?", "帮助", MUTED),
            key("Esc", "再按退出", MUTED),
        ]
    } else {
        match app.current.as_ref().map(|c| c.stage) {
            Some(Stage::Prompt) => vec![
                key("Enter", "揭示", CURRENT),
                key("Space", "发音", MUTED),
                key("Tab", "命令", MUTED),
                key("?", "帮助", MUTED),
                key("Esc", "再按退出", MUTED),
            ],
            _ => {
                let hints = app.interval_hints().unwrap_or_default();
                // Which key just fired? Reverse its chip during the grade flash so the
                // "I pressed 3" moment has shape + color reinforcement (helps color-blind
                // users too — the reversed chip is a structural cue, not just hue).
                let flash_rating = app.grade_flash().map(|(r, _)| r);
                vec![
                    grade_key(
                        "1",
                        "Again",
                        &hints[0],
                        CORAL,
                        "✗",
                        flash_rating == Some(rs_fsrs::Rating::Again),
                    ),
                    grade_key(
                        "2",
                        "Hard",
                        &hints[1],
                        AMBER,
                        "△",
                        flash_rating == Some(rs_fsrs::Rating::Hard),
                    ),
                    grade_key(
                        "3",
                        "Good",
                        &hints[2],
                        GREEN,
                        "○",
                        flash_rating == Some(rs_fsrs::Rating::Good),
                    ),
                    grade_key(
                        "4",
                        "Easy",
                        &hints[3],
                        CURRENT,
                        "✦",
                        flash_rating == Some(rs_fsrs::Rating::Easy),
                    ),
                    sep(),
                    key("↑↓", "选读", CURRENT),
                    key("Space", "发音", MUTED),
                    // The command cluster collapses behind Tab (辨析/词源/星座/设置
                    // all live in the menu) — four extra letter chips pushed the bar
                    // past what a glance can parse, and Tab is the discovery surface.
                    key("Tab", "命令", MUTED),
                    key("?", "帮助", MUTED),
                    key("Esc", "再按退出", MUTED),
                ]
            }
        }
    };
    // The undo window is 3 seconds and invisible — surface it. The chip appears
    // right after a grade (on the next card / done screen) and vanishes when the
    // window closes, which IS the affordance: see it, and `u` still works.
    if app.can_undo() && strike == Strike::Idle && app.derive == DeriveState::Closed {
        let at = spans.len().saturating_sub(1);
        spans.insert(at, key("u", "撤销", AMBER));
    }
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
        app.toast.as_ref().map(|t| {
            let color = match t.level {
                crate::ui::app::ToastLevel::Info => CURRENT,
                crate::ui::app::ToastLevel::Warn => AMBER,
                crate::ui::app::ToastLevel::Error => CORAL,
            };
            Span::styled(format!(" {}   ", t.text), Style::default().fg(color))
        })
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
        Span::styled(
            format!(" {k} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{label} "), Style::default().fg(MUTED)),
    ]
}

fn grade_key(
    k: &str,
    label: &str,
    interval: &str,
    color: ratatui::style::Color,
    mark: &str,
    reverse: bool,
) -> Vec<Span<'static>> {
    let chip_style = if reverse {
        Style::default()
            .fg(ABYSS)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    };
    let label_style = if reverse {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FOAM_DIM)
    };
    vec![
        Span::styled(format!(" {k} "), chip_style),
        Span::styled(format!("{mark} "), Style::default().fg(color)),
        Span::styled(label.to_string(), label_style),
        Span::styled(format!(" {interval}  "), Style::default().fg(MUTED)),
    ]
}

fn sep() -> Vec<Span<'static>> {
    vec![Span::styled("·  ", Style::default().fg(MUTED))]
}

/// The `?` help overlay — a context-sensitive key reference. Grouped so the learner
/// can scan it in seconds; the FSRS group explains what each grade *means* (not just
/// the FSRS jargon), and the interval suffix is decoded (10m = 10 minutes, not months).
fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    if !app.show_help {
        return;
    }
    let group = |heading: &str, rows: &[(&str, &str)]| -> Vec<Line<'static>> {
        let mut v = vec![Line::from(Span::styled(
            heading.to_string(),
            Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
        ))];
        for (k, d) in rows {
            v.push(Line::from(vec![
                Span::styled(format!("  {:<10}", k), Style::default().fg(CURRENT)),
                Span::styled(d.to_string(), Style::default().fg(FOAM_DIM)),
            ]));
        }
        v.push(Line::raw(""));
        v
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.extend(group(
        "导航",
        &[
            ("Enter", "揭示（新词：从拆到联；复习：从问到答）"),
            (
                "↑↓",
                "揭示后选读目标（单词 / 例句）；星座内导航；辨析弹窗滚动",
            ),
            ("Tab", "打开命令菜单（↑↓ 选择，Enter 确认，字母直达）"),
            (
                "Esc",
                "退一层：关浮层 / 跳过星火接线 / 再按一次退出（done 时单按即退）",
            ),
        ],
    ));
    lines.extend(group(
        "发音",
        &[
            ("Space", "朗读当前选中项（单词或例句），只走绑定耳机"),
            ("s", "打开 TTS 引擎设置（Kokoro/Matcha/Piper 运行时切换）"),
        ],
    ));
    lines.extend(group(
        "FSRS 评分（揭示后）",
        &[
            ("1 / h ✗ Again", "忘了 — 重置，很快再考（~1分）"),
            ("2 / j △ Hard", "勉强 — 拉长间隔但标记吃力"),
            ("3 / k ○ Good", "记得 — 正常推进（默认节奏）"),
            ("4 / l ✦ Easy", "轻松 — 大幅拉长间隔"),
            ("u", "撤销上次评分（3 秒内一步；星火接线的锚点复习不回滚）"),
        ],
    ));
    lines.extend(group(
        "星火接线",
        &[
            ("Space", "翻牌：揭示锚点词（已学同根词）"),
            ("y", "记得 — 召回成功，刷新锚点"),
            ("n", "想不起 — 记一次 lapse，诚实标注"),
            ("Esc", "跳过这次接线（不评分，继续评新词）"),
        ],
    ));
    lines.extend(group(
        "扩展",
        &[
            ("a", "新词未揭示：和 AI 一起推导；揭示后与复习：苏格拉底辨析"),
            ("w", "打开 Wiktionary 词源页"),
            ("g", "星座：当前词的词根家族（已学同根 + 一根之差的前沿）"),
            ("?", "本帮助（Esc/? 关闭，其他键穿透到下层）"),
        ],
    ));

    let popup = centered_rect(72, 78, area);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" ? 帮助 · Esc/? 关闭 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

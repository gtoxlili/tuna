//! Rendering. The study screen: a thin status bar (gate + progress), the card,
//! and a context keybar. English/morphemes read as the machine's derivation voice;
//! the ZH meaning you arrive at glows amber.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap};

use super::app::{
    App, CardView, ChatAnchor, ChatMode, ChatState, MORPHEME_CELL_FADE_MS, MORPHEME_STAGGER_MS,
    Stage, Strike,
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

/// Dress a row of spans as the current speak target: a phosphor bar in the left
/// gutter, a soft teal-dark wash under the text, and a closing ♪ marking "Space
/// speaks this line". Unselected rows get the same two-column gutter so text
/// doesn't shift when the cursor moves. The bar + note + wash are structural
/// cues — the selection never rides on hue alone.
fn speak_row(spans: Vec<Span<'_>>, selected: bool) -> Vec<Span<'_>> {
    if !selected {
        let mut out = Vec::with_capacity(spans.len() + 1);
        out.push(Span::raw("  "));
        out.extend(spans);
        return out;
    }
    let mut out = Vec::with_capacity(spans.len() + 2);
    out.push(Span::styled(
        "▎ ",
        Style::default()
            .fg(CURRENT)
            .bg(SPEAK_BG)
            .add_modifier(Modifier::BOLD),
    ));
    for mut s in spans {
        s.style = s.style.bg(SPEAK_BG);
        out.push(s);
    }
    out.push(Span::styled(
        "  ♪",
        Style::default().fg(CURRENT).bg(SPEAK_BG),
    ));
    out
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
    render_chat(frame, area, app);
    render_constellation(frame, area, app);
    settings::render(frame, area, app);
    super::cmdmenu::render(frame, area, app);
    render_primer(frame, area, app);
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

/// The AI chat popup — one overlay, two modes. Derive (pre-reveal): the learner
/// guesses the word's meaning from its morphemes and gets Socratic guidance.
/// Compare (post-reveal): the model opens with the confusable contrast and the
/// learner digs in. Conversation history (scrollable, bottom-pinned) + an input
/// line + a voice-toggle indicator.
fn render_chat(frame: &mut Frame, area: Rect, app: &App) {
    if app.chat == ChatState::Closed {
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    // The line index where the newest AI reply starts — the `LastReply` anchor.
    let mut reply_line: Option<usize> = None;

    // Conversation history — each turn labeled by role.
    for turn in &app.chat_turns {
        if turn.is_user {
            lines.push(Line::from(vec![
                Span::styled("你  ", Style::default().fg(CURRENT)),
                Span::styled(turn.text.clone(), Style::default().fg(FOAM)),
            ]));
        } else {
            let md = tui_markdown::from_str(&turn.text);
            reply_line = Some(lines.len());
            lines.push(Line::from(vec![Span::styled(
                "✦  ",
                Style::default().fg(AMBER),
            )]));
            if md.lines.is_empty() && !turn.text.trim().is_empty() {
                // The markdown parser produced nothing for non-empty text (e.g. a
                // construct the parser build doesn't support) — show the raw text
                // rather than an empty bubble.
                for raw in turn.text.lines() {
                    lines.push(Line::from(Span::styled(
                        raw.to_string(),
                        Style::default().fg(FOAM),
                    )));
                }
            } else {
                lines.extend(md.lines);
            }
        }
        lines.push(Line::raw(""));
    }

    // Input line or pending spinner.
    if app.chat == ChatState::Pending {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", spin(app)), Style::default().fg(CURRENT)),
            Span::styled("思考中……", Style::default().fg(FOAM_DIM)),
        ]));
    } else {
        let placeholder = match app.chat_mode {
            ChatMode::Derive => "说说你看到了哪些词素……",
            ChatMode::Compare => "追问，或说说你的理解……",
            ChatMode::Grammar => "哪里没看懂，直接问……",
        };
        let input_span = if app.input.is_empty() {
            Span::styled(placeholder, Style::default().fg(MUTED))
        } else {
            Span::styled(app.input.clone(), Style::default().fg(FOAM))
        };
        lines.push(Line::from(vec![
            Span::styled("▸  ", Style::default().fg(CURRENT)),
            input_span,
            Span::styled("▋", Style::default().fg(CURRENT)),
        ]));
    }

    // Footer — honest per state: while Pending there is nothing to send. The voice
    // toggle reads as part of the frame's contract, colored by its state.
    lines.push(Line::raw(""));
    let footer = if app.chat == ChatState::Pending {
        "Esc 收起(保留) · ↑↓ 滚动 · "
    } else {
        "Enter 发送 · Esc 收起(保留) · ↑↓ 滚动 · "
    };
    let (voice_label, voice_color) = if app.chat_speak {
        ("♪ 朗读开 Tab", CURRENT)
    } else {
        ("♪ 朗读关 Tab", MUTED)
    };
    lines.push(Line::from(vec![
        Span::styled(footer, Style::default().fg(MUTED)),
        Span::styled(voice_label, Style::default().fg(voice_color)),
    ]));

    let title = match app.chat_mode {
        ChatMode::Derive => " 推导 · 和 AI 一起拆词 ",
        ChatMode::Compare => " 辨析 · 和 AI 分清易混词 ",
        ChatMode::Grammar => " 语法 · 看懂这个句子 ",
    };
    let popup = centered_rect(72, 72, area);

    // Anchor-based viewport. We wrap the lines OURSELVES (`wrap_line`) and render
    // the exact visible slice — Paragraph's word wrap produces an unpredictable
    // row count, and any estimate error here is the input line drifting
    // off-screen. `Bottom` pins the input; `LastReply` starts the view at the
    // newest reply's first row, so a reply taller than the popup is read from its
    // beginning instead of its tail. Manual ↑↓ offsets apply on top, clamped to
    // the real content.
    let inner_w = popup.width.saturating_sub(2 + 4).max(8) as usize; // borders + l/r padding
    let inner_h = popup.height.saturating_sub(2 + 2).max(1) as usize; // borders + t/b padding
    let mut rows: Vec<Line> = Vec::new();
    let mut reply_row = 0usize;
    for (i, l) in lines.iter().enumerate() {
        if reply_line == Some(i) {
            reply_row = rows.len();
        }
        rows.extend(wrap_line(l, inner_w));
    }
    let max_scroll = rows.len().saturating_sub(inner_h);
    let base = match app.chat_anchor {
        ChatAnchor::Bottom => max_scroll,
        ChatAnchor::LastReply => reply_row.min(max_scroll),
    };
    let start = (base as i64 + app.chat_scroll as i64).clamp(0, max_scroll as i64) as usize;
    let visible: Vec<Line> = rows[start..].iter().take(inner_h).cloned().collect();

    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(title)
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Paragraph::new(visible).block(block), popup);
}

/// Greedy display-width wrap of one styled line into rows of at most `width` columns
/// (ASCII = 1 col, everything else = 2). Character-level breaking: predictable row
/// counts are what the chat's bottom-pinned scroll needs, and CJK text wraps at the
/// character anyway. Styles survive the split.
fn wrap_line<'a>(line: &Line<'a>, width: usize) -> Vec<Line<'a>> {
    let width = width.max(4);
    let mut rows: Vec<Line> = Vec::new();
    let mut cur: Vec<Span> = Vec::new();
    let mut cur_w = 0usize;
    for span in &line.spans {
        let style = span.style;
        let mut buf = String::new();
        for ch in span.content.chars() {
            let w = if ch.is_ascii() { 1 } else { 2 };
            if cur_w + w > width {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), style));
                }
                rows.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            }
            buf.push(ch);
            cur_w += w;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
        }
    }
    rows.push(Line::from(cur));
    rows
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
    let selected = c.stage == Stage::Revealed && c.speak_cursor == 0;
    lines.push(Line::from(speak_row(head, selected)));
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
        // Phase B reveal, enriched — answer first, then the derivation refresher.
        (Stage::Revealed, Some(en)) => review_reveal(
            &mut lines,
            c,
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
            // Honest retrieval context for a review card: how many times it's been
            // reviewed and how long ago — orients the recall without leaking it.
            if !c.is_new && c.dc.card.reps > 0 {
                let ago = human_ago(chrono::Utc::now() - c.dc.card.last_review);
                lines.push(Line::from(Span::styled(
                    format!("  第 {} 次复习 · 距上次 {}", c.dc.card.reps, ago),
                    Style::default().fg(MUTED),
                )));
            }
        }
        // Revealed, un-enriched — plain ECDICT meaning + family
        (Stage::Revealed, _) => plain_meaning(&mut lines, c),
    }

    // Derive chat hint (Phase A Prompt): invite the learner to chat with the LLM
    // about the derivation. The chat itself opens via `a` (see render_chat).
    // A collapsed-but-alive conversation changes the invitation to a resumption —
    // the learner should know their thread is still there.
    if c.is_new && matches!(c.stage, Stage::Prompt) {
        lines.push(Line::raw(""));
        if app.chat_turns.is_empty() {
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
                    format!(" 继续推导对话（已 {} 条）", app.chat_turns.len()),
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
    examples_and_confusables(lines, en, speak_cursor);
}

/// Examples (with speak marks — ↑↓ selects, Space speaks) and the confusable
/// pairs. Shared by the phase-A reveal and the review reveal, so a review card
/// never renders a speak cursor pointing at an invisible example.
fn examples_and_confusables(lines: &mut Vec<Line>, en: &Enrichment, speak_cursor: usize) {
    let mut shown = 0;
    for ex in &en.examples {
        if ex.en.is_empty() || shown >= 2 {
            continue;
        }
        let row = vec![
            Span::styled("例  ", Style::default().fg(MUTED)),
            Span::styled(ex.en.clone(), Style::default().fg(FOAM)),
        ];
        lines.push(Line::raw(""));
        lines.push(Line::from(speak_row(row, speak_cursor == shown + 1)));
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

/// Compact Chinese "time since" for the review-prompt context line (分/时/天,
/// mirroring the grade-hint units).
fn human_ago(d: chrono::Duration) -> String {
    let mins = d.num_minutes().max(0);
    if mins < 60 {
        format!("{}分", mins.max(1))
    } else if mins < 60 * 24 {
        format!("{}时", mins / 60)
    } else {
        format!("{}天", mins / (60 * 24))
    }
}

/// Split an ECDICT translation line into its part-of-speech tag and the meaning
/// ("vt. 陈述" → ("vt.", "陈述")), so the tag can render dim and the meaning carry
/// the color. Lines without a leading tag pass through whole.
fn pos_split(line: &str) -> (Option<&str>, &str) {
    if let Some((head, rest)) = line.split_once(' ')
        && head.len() <= 8
        && head.ends_with('.')
        && head
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || matches!(ch, '.' | '&' | '/'))
    {
        return (Some(head), rest.trim_start());
    }
    (None, line)
}

/// ECDICT translation lines with the POS tag dimmed into a fixed-width gutter,
/// so the meanings align in a column instead of reading as a wall of amber.
fn translation_rows(lines: &mut Vec<Line>, translation: &str, cap: usize, lead: Style, rest: Style) {
    for (i, t) in translation
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .take(cap)
        .enumerate()
    {
        let (pos, meaning) = pos_split(t);
        let style = if i == 0 { lead } else { rest };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<5}", pos.unwrap_or("")),
                Style::default().fg(MUTED),
            ),
            Span::styled(meaning.to_string(), style),
        ]));
    }
}

/// The review reveal (Phase B 验, enriched): the ANSWER leads — the learner just
/// tried to retrieve it — then a one-glance refresher of the derivation scaffold,
/// then examples/confusables with the speak cursor live.
fn review_reveal(
    lines: &mut Vec<Line>,
    c: &CardView,
    en: &Enrichment,
    speak_cursor: usize,
    reveal_ms: Option<u128>,
    owned: &[String],
) {
    if !en.gloss_zh.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("释义  ", Style::default().fg(MUTED)),
            Span::styled(
                en.gloss_zh.clone(),
                Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    let lead = if en.gloss_zh.is_empty() {
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FOAM_DIM)
    };
    translation_rows(
        lines,
        &c.entry.translation,
        3,
        lead,
        Style::default().fg(FOAM_DIM),
    );
    // The scaffold, refreshed in one glance: the parts and the chain that derived it.
    if !en.morphemes.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(morpheme_cells(en, reveal_ms, owned)));
    }
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
                Style::default().fg(AMBER)
            } else {
                Style::default().fg(FOAM_DIM)
            };
            spans.push(Span::styled(p.to_string(), style));
        }
        lines.push(Line::from(spans));
    }
    examples_and_confusables(lines, en, speak_cursor);
}

/// Plain meaning for un-enriched cards: POS-guttered translations + EN definition
/// + ECDICT word-family.
fn plain_meaning(lines: &mut Vec<Line>, c: &CardView) {
    translation_rows(
        lines,
        &c.entry.translation,
        4,
        Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        Style::default().fg(AMBER),
    );
    if !c.entry.definition.is_empty() {
        lines.push(Line::raw(""));
        for (i, d) in c
            .entry
            .definition
            .lines()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .take(2)
            .enumerate()
        {
            let label = if i == 0 { "英释 " } else { "     " };
            lines.push(Line::from(vec![
                Span::styled(label, Style::default().fg(MUTED)),
                Span::styled(d.to_string(), Style::default().fg(FOAM_DIM)),
            ]));
        }
    }
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
    let mut spans = if app.chat != ChatState::Closed {
        // Chat input owns the keyboard — the base shortcuts underneath are dead,
        // so showing them would be a lie. Mirror the popup's footer.
        let voice = key(
            "Tab",
            if app.chat_speak { "朗读开" } else { "朗读关" },
            if app.chat_speak { CURRENT } else { MUTED },
        );
        if app.chat == ChatState::Pending {
            vec![key("↑↓", "滚动", MUTED), voice, key("Esc", "收起", MUTED)]
        } else {
            vec![
                key("Enter", "发送", CURRENT),
                key("⌫", "删字", MUTED),
                key("↑↓", "滚动", MUTED),
                voice,
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
                    key("↑↓", "选中", CURRENT),
                    key("Space", "发音", MUTED),
                    // `a` follows the cursor: pointing at an example asks about THAT
                    // sentence's grammar, pointing at the word asks the contrast.
                    key(
                        "a",
                        if app.selected_example().is_some() {
                            "析这句"
                        } else {
                            "辨析"
                        },
                        AMBER,
                    ),
                    // The other commands collapse behind Tab (词源/星座/设置 live in
                    // the menu) — extra letter chips pushed the bar past what a
                    // glance can parse, and Tab is the discovery surface.
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
    if app.can_undo() && strike == Strike::Idle && app.chat == ChatState::Closed {
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
                "揭示后选中朗读/提问目标（单词 / 例句）；星座内导航；对话/帮助滚动",
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
            ("Tab(对话内)", "切换 AI 回复朗读（中英混合语音，需下载模型）"),
        ],
    ));
    lines.extend(group(
        "FSRS 评分（揭示后，给当前这张卡打分）",
        &[
            ("1 / h ✗ Again", "忘了 — 重置，很快再考"),
            ("2 / j △ Hard", "勉强 — 拉长间隔但标记吃力"),
            ("3 / k ○ Good", "记得 — 正常推进（默认节奏）"),
            ("4 / l ✦ Easy", "轻松 — 大幅拉长间隔"),
            ("u", "撤销上次评分（3 秒内一步；星火接线的锚点复习不回滚）"),
        ],
    ));
    lines.extend(group(
        "星火接线（自动出现，给已学的旧词加一次复习）",
        &[
            (
                "触发",
                "新词揭示后，若它与某个已学过的词共享词根，卡片上自动出现；刚开始学、还没有已学同根词时不会出现",
            ),
            ("Space", "翻牌：揭示那个已学的锚点词"),
            ("y / n", "记得 / 想不起 — 给旧词记一次真实 FSRS 复习（翻牌后生效）"),
            ("Esc", "跳过这次接线（旧词不评分，直接评新词）"),
        ],
    ));
    lines.extend(group(
        "扩展",
        &[
            (
                "a",
                "问 AI：新词未揭示=推导；选中例句=讲这句的语法；其余=易混辨析",
            ),
            ("x", "语法速查：词性缩写与句子骨架的大白话说明（离线）"),
            ("w", "打开 Wiktionary 词源页"),
            ("g", "星座：当前词的词根家族（已学同根 + 一根之差的前沿）"),
            ("?", "本帮助（↑↓ 滚动，Esc/? 关闭，其他键穿透到下层）"),
        ],
    ));

    let popup = centered_rect(72, 78, area);
    // Exact-slice scrolling (same wrap as the chat popup): the reference outgrows a
    // 24-row terminal, and Paragraph-side wrap would make the clamp a guess.
    let inner_w = popup.width.saturating_sub(2 + 4).max(8) as usize;
    let inner_h = popup.height.saturating_sub(2 + 2).max(1) as usize;
    let rows: Vec<Line> = lines.iter().flat_map(|l| wrap_line(l, inner_w)).collect();
    let max_scroll = rows.len().saturating_sub(inner_h);
    let scroll = app.help_scroll.min(max_scroll);
    let mut visible: Vec<Line> = rows[scroll..].iter().take(inner_h).cloned().collect();
    // Overflow indicator on the last visible row — an invisible tail reads as "that's
    // all" without it.
    if scroll < max_scroll
        && let Some(last) = visible.last_mut()
    {
        *last = Line::from(Span::styled(
            "   ⌄ ↓ 更多",
            Style::default().fg(CURRENT),
        ));
    }
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(CURRENT))
        .title(" ? 帮助 · ↑↓ 滚动 · Esc/? 关闭 ")
        .title_style(Style::default().fg(CURRENT).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Paragraph::new(visible).block(block), popup);
}

/// The offline grammar primer (`x`) — a survival glossary, not a course: it
/// decodes exactly the grammar surface tuna itself shows (the POS gutter on
/// review cards, why example sentences need the little words) in plain language.
/// Scrolls like the help overlay.
fn render_primer(frame: &mut Frame, area: Rect, app: &App) {
    if !app.show_primer {
        return;
    }
    let head = |t: &str| -> Line<'static> {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(FOAM).add_modifier(Modifier::BOLD),
        ))
    };
    let row = |k: &str, d: &str| -> Line<'static> {
        Line::from(vec![
            Span::styled(format!("  {:<7}", k), Style::default().fg(CURRENT)),
            Span::styled(d.to_string(), Style::default().fg(FOAM_DIM)),
        ])
    };
    let note = |t: &str| -> Line<'static> {
        Line::from(Span::styled(
            format!("  {t}"),
            Style::default().fg(FOAM_DIM),
        ))
    };
    let lines: Vec<Line> = vec![
        head("词性 — 词的身份（释义前的缩写就是它）"),
        row("n.", "名词：人、物、概念的名字（state 国家）"),
        row("v.", "动词：动作或状态（push 推）"),
        row("vt.", "及物动词：后面直接跟对象（state a fact 陈述事实）"),
        row("vi.", "不及物动词：不能直接跟对象，要接就得借介词（lean against the wall）"),
        row("adj./a.", "形容词：描述名词（mighty storm 猛烈的风暴）"),
        row("adv./ad.", "副词：描述动作怎么发生（push hard 用力推）"),
        row("prep.", "介词：挂名词用的小词，表示关系（against / with / in）"),
        row("conj.", "连词：把两句话或两个成分连起来（and / but / because）"),
        row("pron.", "代词：替名词出场（he / it / this）"),
        row("art.", "冠词：名词前的小标记（a / an / the）"),
        Line::raw(""),
        head("句子骨架 — 每个英语句子的底层结构"),
        note("谁 + 做什么 + 对什么：He pushed the car.（他 推 车）"),
        note("是什么/怎么样：The evidence is against him.（主语 + be + 描述）"),
        note("其余都是挂件：时间、地点、方式，用介词短语挂上去"),
        Line::raw(""),
        head("为什么需要介词"),
        note("每个动词能不能直接带对象是固定的。能直接带的叫及物（push the car）；"),
        note("不能的叫不及物，要表达对象就得用介词搭桥：lean 靠（不及物）"),
        note("→ lean the wall ✗ → lean against the wall ✓（against 搭的桥）"),
        Line::raw(""),
        head("时态在动词上变形"),
        note("过去发生 → 动词变形：push → pushed；is → was"),
        note("正在进行 → be + 动词-ing：is pushing"),
        note("已经完成 → have + 过去分词：has pushed"),
        Line::raw(""),
        head("看长句的顺序"),
        note("1 先找谓语动词（句子的心脏）  2 谓语前面是主语"),
        note("3 谓语后面是对象/描述  4 剩下的介词短语、从句逐个挂回去"),
        Line::raw(""),
        note("例句里哪里没看懂：↑↓ 选中那句，按 a 让 AI 讲给你听"),
    ];
    // Chat-style exact slicing so the tail is always reachable.
    let popup = centered_rect(76, 80, area);
    let inner_w = popup.width.saturating_sub(2 + 4).max(8) as usize;
    let inner_h = popup.height.saturating_sub(2 + 2).max(1) as usize;
    let rows: Vec<Line> = lines.iter().flat_map(|l| wrap_line(l, inner_w)).collect();
    let max_scroll = rows.len().saturating_sub(inner_h);
    let scroll = app.primer_scroll.min(max_scroll);
    let mut visible: Vec<Line> = rows[scroll..].iter().take(inner_h).cloned().collect();
    if scroll < max_scroll
        && let Some(last) = visible.last_mut()
    {
        *last = Line::from(Span::styled(
            "   ⌄ ↓ 更多",
            Style::default().fg(CURRENT),
        ));
    }
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(AMBER))
        .title(" x 语法速查 · ↑↓ 滚动 · Esc/x 关闭 ")
        .title_style(Style::default().fg(AMBER).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(SLATE).fg(FOAM))
        .padding(Padding::new(2, 2, 1, 1));
    frame.render_widget(Paragraph::new(visible).block(block), popup);
}

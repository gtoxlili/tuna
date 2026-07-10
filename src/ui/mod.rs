//! The study TUI: terminal lifecycle + the synchronous render/event loop.

pub mod app;
pub mod cmdmenu;
pub mod settings;
pub mod theme;
pub mod view;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;

use crate::config::Config;

pub fn run(deck_path: &Path) -> Result<()> {
    let cfg = Config::load()?;
    let mut app = App::new(deck_path, &cfg)?;

    // Guard: an unbuilt deck is a bug state (ensure_ready rebuilds from assets on
    // every launch). If we get here, the embedded deck asset itself is empty.
    if app.deck.stats()?.cards == 0 {
        println!("\n  牌组为空，内嵌词库可能损坏。请重新安装或到 GitHub 反馈。\n");
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    res
}

/// Render the study screen to an in-memory buffer and print it as text — lets us
/// verify layout/content without an interactive TTY. Shows both card stages.
pub fn preview(deck_path: &Path, word: Option<String>) -> Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let cfg = Config::load()?;
    let mut app = App::new(deck_path, &cfg)?;
    if app.deck.stats()?.cards == 0 {
        println!("deck empty — run `tuna build-deck` first");
        return Ok(());
    }
    if let Some(w) = word
        && !app.force_card(&w)? {
            println!("'{w}' not in deck");
            return Ok(());
        }
    let mut term = Terminal::new(TestBackend::new(96, 32))?;

    term.draw(|f| view::render(f, &app))?;
    println!("── PROMPT ──\n{}", term.backend());

    if let Some(c) = app.current.as_mut() {
        c.stage = app::Stage::Revealed;
        if c.anchor.is_some() {
            c.strike = app::Strike::Prompt;
        }
    }
    term.draw(|f| view::render(f, &app))?;
    println!(
        "\n── REVEALED (+ 星火接线 prompt if anchor) ──\n{}",
        term.backend()
    );

    if app
        .current
        .as_ref()
        .map(|c| c.anchor.is_some())
        .unwrap_or(false)
    {
        if let Some(c) = app.current.as_mut() {
            c.strike = app::Strike::Flipped;
        }
        term.draw(|f| view::render(f, &app))?;
        println!("\n── STRIKE FLIPPED (recall check) ──\n{}", term.backend());

        // arc firing mid-animation (simulate a recall success ~half done). Window is
        // now 400ms (was 900), so 200ms lands at the midpoint of the arc.
        if let Some(c) = app.current.as_mut() {
            c.strike = app::Strike::Idle;
        }
        app.strike_anim = Some(std::time::Instant::now() - std::time::Duration::from_millis(200));
        term.draw(|f| view::render(f, &app))?;
        println!("\n── STRIKE ARC (mid-fire) ──\n{}", term.backend());
    }

    // Verify the compare chat renders markdown AND that a reply taller than the
    // popup is presented from its FIRST line (LastReply anchor) — bottom-pinned it
    // would show only the tail, which is exactly the "回复被吞" bug.
    app.chat = app::ChatState::Open;
    app.chat_mode = app::ChatMode::Compare;
    app.chat_anchor = app::ChatAnchor::LastReply;
    let long_reply = format!(
        "先分别拆开词根：\n- **transport**：trans-（跨）+ port（携带）\n- **transit**：trans-（跨）+ it（走）\n{}\n提问：运送货物与自身穿越，语义上会导向怎样的不同？",
        "对比在真实语料里的分布，运送对象与自身移动的差别会不断重现。\n".repeat(14)
    );
    app.chat_turns.push(app::ChatTurn {
        is_user: false,
        text: long_reply,
    });
    term.draw(|f| view::render(f, &app))?;
    println!("\n── COMPARE CHAT (long reply, anchored to top) ──\n{}", term.backend());

    // Verify the derive chat popup: history + input line pinned to the bottom edge.
    app.chat_turns.clear();
    if let Some(c) = app.current.as_mut() {
        c.stage = app::Stage::Prompt;
    }
    app.chat_mode = app::ChatMode::Derive;
    app.chat_turns.push(app::ChatTurn {
        is_user: true,
        text: "我看到 spect 是看".to_string(),
    });
    app.chat_turns.push(app::ChatTurn {
        is_user: false,
        text: "对，spect 抓对了。再看词尾 -ate，它常把词变成什么词性？".to_string(),
    });
    app.input = "动词?".to_string();
    term.draw(|f| view::render(f, &app))?;
    println!("\n── DERIVE CHAT (input pinned) ──\n{}", term.backend());

    // Verify the review (Phase B) panels: retrieval context on the prompt,
    // answer-first layout on the reveal.
    app.chat = app::ChatState::Closed;
    app.chat_turns.clear();
    app.input.clear();
    if let Some(c) = app.current.as_mut() {
        c.is_new = false;
        c.stage = app::Stage::Prompt;
        c.dc.card.reps = 3;
        c.dc.card.last_review = chrono::Utc::now() - chrono::Duration::days(3);
    }
    term.draw(|f| view::render(f, &app))?;
    println!("\n── REVIEW PROMPT (retrieval context) ──\n{}", term.backend());

    if let Some(c) = app.current.as_mut() {
        c.stage = app::Stage::Revealed;
    }
    term.draw(|f| view::render(f, &app))?;
    println!("\n── REVIEW REVEALED (answer first) ──\n{}", term.backend());
    Ok(())
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| view::render(f, app))?;

        // Redraw fast while something is animating (slide-in / stagger / spinner), else
        // idle-poll so the earphone gate stays live without burning CPU. 50ms ≈ 20fps,
        // smooth enough for the 150ms slide-in and 60ms stagger steps.
        let timeout = if app.is_animating() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        if event::poll(timeout)?
            && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press {
                    app.on_key(k)?;
                }
        app.poll_gate();
        app.poll_async();
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

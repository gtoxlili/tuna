//! The study TUI: terminal lifecycle + the synchronous render/event loop.

pub mod app;
pub mod theme;
pub mod view;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use app::App;

use crate::config::Config;

pub fn run(deck_path: &Path) -> Result<()> {
    let cfg = Config::load()?;
    let mut app = App::new(deck_path, &cfg)?;

    // Guard: an unbuilt deck should point the user at `build-deck`, not show
    // a hollow "session complete" screen.
    if app.deck.stats()?.cards == 0 {
        println!("\n  牌组为空。先构建它：\n    cargo run -- build-deck\n");
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let cfg = Config::load()?;
    let mut app = App::new(deck_path, &cfg)?;
    if app.deck.stats()?.cards == 0 {
        println!("deck empty — run `tuna build-deck` first");
        return Ok(());
    }
    if let Some(w) = word {
        if !app.force_card(&w)? {
            println!("'{w}' not in deck");
            return Ok(());
        }
    }
    let mut term = Terminal::new(TestBackend::new(96, 32))?;

    app.input = "圈起来 → 限制".to_string();
    term.draw(|f| view::render(f, &app))?;
    println!("── PROMPT (derive game) ──\n{}", term.backend());

    if let Some(c) = app.current.as_mut() {
        c.stage = app::Stage::Revealed;
        if c.anchor.is_some() {
            c.strike = app::Strike::Prompt;
        }
    }
    term.draw(|f| view::render(f, &app))?;
    println!("\n── REVEALED (+ 星火接线 prompt if anchor) ──\n{}", term.backend());

    if app.current.as_ref().map(|c| c.anchor.is_some()).unwrap_or(false) {
        if let Some(c) = app.current.as_mut() {
            c.strike = app::Strike::Flipped;
        }
        term.draw(|f| view::render(f, &app))?;
        println!("\n── STRIKE FLIPPED (recall check) ──\n{}", term.backend());
    }

    // Verify the Socratic popup renders markdown (bold/lists), not raw syntax.
    app.ask = app::Ask::Answer(
        "先分别拆开词根：\n- **transport**：trans-（跨）+ port（携带）\n- **transit**：trans-（跨）+ it（走）\n\n提问：运送货物与自身穿越，语义上会导向怎样的不同？\n\n核心差异：**transport** 强调把对象运到另一处；**transit** 只突出经过、中转。"
            .to_string(),
    );
    term.draw(|f| view::render(f, &app))?;
    println!("\n── SOCRATIC POPUP (markdown) ──\n{}", term.backend());
    Ok(())
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| view::render(f, app))?;

        // Redraw fast while something is animating (spinner), else idle-poll so the
        // earphone gate stays live without burning CPU.
        let timeout = if app.is_animating() {
            Duration::from_millis(80)
        } else {
            Duration::from_millis(200)
        };
        if event::poll(timeout)? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    let ch = match k.code {
                        KeyCode::Char(c) => Some(c),
                        KeyCode::Enter => Some('\n'),
                        KeyCode::Esc => Some('\x1b'),
                        KeyCode::Backspace => Some('\x08'),
                        _ => None,
                    };
                    if let Some(c) = ch {
                        app.on_key(c)?;
                    }
                }
            }
        }
        app.poll_gate();
        app.poll_async();
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

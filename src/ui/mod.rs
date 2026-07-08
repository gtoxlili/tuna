//! The study TUI: terminal lifecycle + the synchronous render/event loop.

pub mod app;
pub mod theme;
pub mod view;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use app::App;

pub fn run(deck_path: &Path, needle: String) -> Result<()> {
    let mut app = App::new(deck_path, needle)?;

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
pub fn preview(deck_path: &Path, needle: String) -> Result<()> {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut app = App::new(deck_path, needle)?;
    if app.deck.stats()?.cards == 0 {
        println!("deck empty — run `tuna build-deck` first");
        return Ok(());
    }
    let mut term = Terminal::new(TestBackend::new(96, 20))?;

    term.draw(|f| view::render(f, &app))?;
    println!("── PROMPT ──\n{}", term.backend());

    if let Some(c) = app.current.as_mut() {
        c.stage = app::Stage::Revealed;
    }
    term.draw(|f| view::render(f, &app))?;
    println!("\n── REVEALED ──\n{}", term.backend());
    Ok(())
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| view::render(f, app))?;

        // Poll with a timeout so the earphone gate stays live even when idle.
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    let ch = match k.code {
                        KeyCode::Char(c) => Some(c),
                        KeyCode::Enter => Some('\n'),
                        KeyCode::Esc => Some('q'),
                        _ => None,
                    };
                    if let Some(c) = ch {
                        app.on_key(c)?;
                    }
                }
            }
        }
        app.poll_gate();
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

use std::sync::mpsc::Sender;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};

pub enum KeyEvent {
    Quit,
}

/// Polls crossterm keyboard events and forwards quit signals to the UI thread.
pub fn keyboard_loop(tx: Sender<KeyEvent>) {
    loop {
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL));
                if quit {
                    let _ = tx.send(KeyEvent::Quit);
                    return;
                }
            }
        }
    }
}

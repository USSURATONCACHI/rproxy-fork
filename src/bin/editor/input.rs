use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, EditField, Mode};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Global: Ctrl+C always quits
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    match &app.mode {
        Mode::Normal => handle_normal(app, key),
        Mode::Edit | Mode::Insert => handle_edit(app, key),
        Mode::Command => handle_command(app, key),
        Mode::ConfirmDelete => handle_confirm_delete(app, key),
        Mode::Help => handle_help(app, key),
    }
}

fn handle_normal(app: &mut App, key: KeyEvent) {
    app.message = None;
    match key.code {
        KeyCode::Char('q') => {
            if app.dirty {
                app.message = Some("Unsaved changes! Use :q! to force quit or :wq to save and quit.".into());
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('j') | KeyCode::Down => app.move_down(),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(),
        KeyCode::Char('g') => app.move_to_top(),
        KeyCode::Char('G') => app.move_to_bottom(),
        KeyCode::Char('i') | KeyCode::Enter => app.enter_edit(),
        KeyCode::Char('a') => app.enter_insert(),
        KeyCode::Char('d') => app.request_delete(),
        KeyCode::Char(':') => app.enter_command(),
        KeyCode::Char('?') => {
            app.mode = Mode::Help;
        }
        _ => {}
    }
}

fn handle_edit(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.message = None;
        }
        KeyCode::Tab => app.next_field(),
        KeyCode::BackTab => app.prev_field(),
        KeyCode::Enter => {
            if let Err(e) = app.confirm_edit() {
                app.message = Some(e);
            }
        }
        KeyCode::Backspace => app.delete_char(),
        KeyCode::Left => app.cursor_left(),
        KeyCode::Right => app.cursor_right(),
        KeyCode::Char(' ') | KeyCode::Char('\t') => {
            if app.edit_field == EditField::Protocol {
                app.toggle_protocol();
            } else {
                app.insert_char(' ');
            }
        }
        KeyCode::Char(c) => {
            if app.edit_field == EditField::Protocol {
                app.toggle_protocol();
            } else {
                app.insert_char(c);
            }
        }
        _ => {}
    }
}

fn handle_command(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Normal;
            app.message = None;
        }
        KeyCode::Enter => {
            app.execute_command();
        }
        KeyCode::Backspace => {
            if app.command_buffer.is_empty() {
                app.mode = Mode::Normal;
            } else {
                app.command_buffer.pop();
            }
        }
        KeyCode::Char(c) => {
            app.command_buffer.push(c);
        }
        _ => {}
    }
}

fn handle_confirm_delete(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete(),
        _ => app.cancel_delete(),
    }
}

fn handle_help(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            app.mode = Mode::Normal;
            app.message = None;
        }
        _ => {}
    }
}

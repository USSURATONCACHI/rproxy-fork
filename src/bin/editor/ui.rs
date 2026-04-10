use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

use super::app::{App, EditField, Mode, SettingsField};

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Min(5),   // table
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    render_title(frame, app, chunks[0]);
    render_table(frame, app, chunks[1]);
    render_status_bar(frame, app, chunks[2]);

    // Overlays
    match app.mode {
        Mode::Edit | Mode::Insert => render_edit_popup(frame, app),
        Mode::Settings => render_settings_popup(frame, app),
        Mode::Help => render_help_popup(frame),
        _ => {}
    }
}

fn render_title(frame: &mut Frame, app: &App, area: Rect) {
    let dirty_marker = if app.dirty { " [+]" } else { "" };
    let title = format!(
        " rproxy config editor — {}{}",
        app.config_path.display(),
        dirty_marker
    );
    let paragraph = Paragraph::new(title).style(
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(paragraph, area);
}

fn render_table(frame: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        Cell::from(" # "),
        Cell::from(" Bind"),
        Cell::from(" Remote"),
        Cell::from(" Proto"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let rows: Vec<Row> = app
        .proxies
        .iter()
        .enumerate()
        .map(|(i, proxy)| {
            let marker = if i == app.selected && app.mode == Mode::Normal {
                ">"
            } else {
                " "
            };
            let num = format!("{}{}", marker, i + 1);
            let style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(num),
                Cell::from(format!(" {}", proxy.bind)),
                Cell::from(format!(" {}", proxy.remote)),
                Cell::from(format!(" {}", proxy.protocol)),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(5),
        Constraint::Percentage(35),
        Constraint::Percentage(45),
        Constraint::Length(7),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::NONE));

    frame.render_widget(table, area);
}

fn render_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let content = match &app.mode {
        Mode::Normal => {
            if let Some(msg) = &app.message {
                msg.clone()
            } else {
                " [NORMAL] j/k:nav  i:edit  a:add  d:del  s:settings  ::cmd  ?:help  q:quit".into()
            }
        }
        Mode::Edit => " [EDIT] Tab:next field  Enter:confirm  Esc:cancel".into(),
        Mode::Insert => " [INSERT] Tab:next field  Enter:confirm  Esc:cancel".into(),
        Mode::Command => format!(" :{}", app.command_buffer),
        Mode::ConfirmDelete => app
            .message
            .clone()
            .unwrap_or_else(|| " Delete? y/n".into()),
        Mode::Settings => " [SETTINGS] Tab:next field  Enter:confirm  Esc:cancel".into(),
        Mode::Help => " [HELP] Press Esc or q to close".into(),
    };

    let style = match app.mode {
        Mode::Normal => Style::default().bg(Color::DarkGray).fg(Color::White),
        Mode::Edit | Mode::Insert => Style::default().bg(Color::Green).fg(Color::Black),
        Mode::Settings => Style::default().bg(Color::Cyan).fg(Color::Black),
        Mode::Command => Style::default().bg(Color::DarkGray).fg(Color::White),
        Mode::ConfirmDelete => Style::default().bg(Color::Red).fg(Color::White),
        Mode::Help => Style::default().bg(Color::Magenta).fg(Color::White),
    };

    // Show error messages in red
    let style = if let Some(msg) = &app.message {
        if msg.starts_with("Error") || msg.contains("must be") || msg.contains("cannot") || msg.contains("expected") {
            Style::default().bg(Color::Red).fg(Color::White)
        } else {
            style
        }
    } else {
        style
    };

    let paragraph = Paragraph::new(content).style(style);
    frame.render_widget(paragraph, area);
}

fn render_edit_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let title = match app.mode {
        Mode::Edit => format!("Edit Proxy #{}", app.selected + 1),
        Mode::Insert => "New Proxy".into(),
        _ => "".into(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Bind
            Constraint::Length(2), // Remote
            Constraint::Length(2), // Protocol
            Constraint::Length(1), // spacer
            Constraint::Length(1), // help
        ])
        .split(inner);

    render_field(frame, "Bind:     ", &app.edit_bind, app.edit_field == EditField::Bind, field_chunks[0], app);
    render_field(frame, "Remote:   ", &app.edit_remote, app.edit_field == EditField::Remote, field_chunks[1], app);
    render_protocol_field(frame, &app.edit_protocol, app.edit_field == EditField::Protocol, field_chunks[2]);

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Yellow)),
        Span::raw(": confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(": cancel  "),
        Span::styled("Tab", Style::default().fg(Color::Yellow)),
        Span::raw(": next field"),
    ]));
    frame.render_widget(help, field_chunks[4]);
}

fn render_field(frame: &mut Frame, label: &str, value: &str, active: bool, area: Rect, app: &App) {
    let style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Gray)
    };

    let input_style = if active {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    } else {
        Style::default()
    };

    let line = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(12), Constraint::Min(1)])
        .split(area);

    let label_widget = Paragraph::new(format!(" {}", label)).style(style);
    frame.render_widget(label_widget, line[0]);

    let display_value = if active {
        // Show cursor
        let pos = app.cursor_pos;
        let mut display = value.to_string();
        if pos >= display.len() {
            display.push('_');
        }
        display
    } else {
        value.to_string()
    };

    let value_widget = Paragraph::new(format!(" {}", display_value)).style(input_style);
    frame.render_widget(value_widget, line[1]);
}

fn render_protocol_field(frame: &mut Frame, value: &str, active: bool, area: Rect) {
    let style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Gray)
    };

    let input_style = if active {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    } else {
        Style::default()
    };

    let line = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(12), Constraint::Min(1)])
        .split(area);

    let label_widget = Paragraph::new(" Protocol: ").style(style);
    frame.render_widget(label_widget, line[0]);

    let hint = if active { "  (Space/any key to toggle)" } else { "" };
    let value_widget = Paragraph::new(format!(" {}{}", value, hint)).style(input_style);
    frame.render_widget(value_widget, line[1]);
}

fn render_settings_popup(frame: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Settings")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // MaxConnections
            Constraint::Length(2), // MaxClientTunnels
            Constraint::Length(2), // KeepaliveIdle
            Constraint::Length(2), // KeepaliveInterval
            Constraint::Length(1), // spacer
            Constraint::Length(1), // help
        ])
        .split(inner);

    let fields = [
        (SettingsField::MaxConnections, app.settings.max_connections.to_string()),
        (SettingsField::MaxClientTunnels, app.settings.max_client_tunnels.to_string()),
        (SettingsField::KeepaliveIdle, app.settings.keepalive_idle.to_string()),
        (SettingsField::KeepaliveInterval, app.settings.keepalive_interval.to_string()),
    ];

    for (idx, (field, default_val)) in fields.iter().enumerate() {
        let active = app.settings_field == *field;
        let display_val = if active {
            let mut v = app.settings_buffer.clone();
            if app.settings_cursor >= v.len() {
                v.push('_');
            }
            v
        } else {
            default_val.clone()
        };

        let label_style = if active {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::Gray)
        };
        let input_style = if active {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default()
        };

        let row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(1)])
            .split(field_chunks[idx]);

        let label = Paragraph::new(format!(" {}:", field.label())).style(label_style);
        frame.render_widget(label, row[0]);

        let value = Paragraph::new(format!(" {}", display_val)).style(input_style);
        frame.render_widget(value, row[1]);
    }

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Yellow)),
        Span::raw(": confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(": cancel  "),
        Span::styled("Tab", Style::default().fg(Color::Yellow)),
        Span::raw(": next field"),
    ]));
    frame.render_widget(help, field_chunks[5]);
}

fn render_help_popup(frame: &mut Frame) {
    let area = centered_rect(65, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title("Help — Keybindings")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let help_text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Normal Mode", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
        ]),
        Line::from("    j / Down      Move down"),
        Line::from("    k / Up        Move up"),
        Line::from("    g             Jump to top"),
        Line::from("    G             Jump to bottom"),
        Line::from("    i / Enter     Edit selected entry"),
        Line::from("    a             Add new entry"),
        Line::from("    d             Delete selected entry"),
        Line::from("    s             Edit settings"),
        Line::from("    :             Enter command mode"),
        Line::from("    ?             Show this help"),
        Line::from("    q             Quit"),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Edit / Insert Mode", Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)),
        ]),
        Line::from("    Tab           Next field"),
        Line::from("    Shift+Tab     Previous field"),
        Line::from("    Enter         Confirm"),
        Line::from("    Esc           Cancel"),
        Line::from("    Space         Toggle protocol (on Protocol field)"),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Commands", Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow)),
        ]),
        Line::from("    :w            Save"),
        Line::from("    :w <path>     Save to path"),
        Line::from("    :q            Quit (fails if unsaved)"),
        Line::from("    :q!           Force quit"),
        Line::from("    :wq           Save and quit"),
        Line::from(""),
        Line::from("  Press Esc or q to close this help."),
    ];

    let paragraph = Paragraph::new(help_text).block(block);
    frame.render_widget(paragraph, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

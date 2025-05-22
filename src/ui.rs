use genai_types::{messages::Role, MessageContent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        Wrap,
    },
    Frame,
};

use crate::app::{App, InputMode};
use crate::config::Args;

/// Render the main user interface
pub fn render(f: &mut Frame, app: &mut App, args: &Args) {
    let size = f.area();

    // Create main layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),       // Title bar
            Constraint::Min(1),          // Chat area
            Constraint::Length(3),       // Input area
            Constraint::Length(1),       // Status bar
        ])
        .split(size);

    // Title bar
    render_title_bar(f, chunks[0], args);

    // Chat messages area
    render_chat_area(f, chunks[1], app);

    // Input area
    render_input_area(f, chunks[2], app);

    // Status bar
    render_status_bar(f, chunks[3], app, args);

    // Help popup (rendered on top if active)
    if app.show_help {
        render_help_popup(f, size);
    }
}

/// Render the title bar
fn render_title_bar(f: &mut Frame, area: ratatui::layout::Rect, args: &Args) {
    let title = format!("🤖 th-chat - {}", args.title);
    let title_paragraph = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center);
    f.render_widget(title_paragraph, area);
}

/// Render the chat messages area
fn render_chat_area(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let messages_block = Block::default()
        .borders(Borders::ALL)
        .title("Chat")
        .title_style(Style::default().fg(Color::Yellow));

    let messages: Vec<ListItem> = app
        .messages
        .iter()
        .enumerate()
        .skip(app.vertical_scroll)
        .map(|(_, chat_msg)| {
            let content = match &chat_msg.message.content[0] {
                MessageContent::Text { text } => text.clone(),
                _ => "Unsupported message type".to_string(),
            };

            let (prefix, style) = match chat_msg.message.role {
                Role::User => ("👤 You: ", Style::default().fg(Color::Green)),
                Role::Assistant => ("🤖 Assistant: ", Style::default().fg(Color::Blue)),
                Role::System => ("⚙️  System: ", Style::default().fg(Color::Yellow)),
            };

            let wrapped_content = textwrap::fill(&content, (area.width - 4) as usize);
            let mut lines = vec![Line::from(Span::styled(
                format!("{}{}", prefix, wrapped_content.lines().next().unwrap_or("")),
                style,
            ))];

            // Add continuation lines for wrapped text
            for line in wrapped_content.lines().skip(1) {
                lines.push(Line::from(Span::styled(
                    format!("     {}", line),
                    style,
                )));
            }

            ListItem::new(lines)
        })
        .collect();

    let messages_list = List::new(messages).block(messages_block);
    f.render_widget(messages_list, area);

    // Render scrollbar for messages
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));
    f.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut app.scroll_state,
    );
}

/// Render the input area
fn render_input_area(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(match app.input_mode {
            InputMode::Normal => "Input (Press 'i' to edit, 'q' to quit, F1 for help)",
            InputMode::Editing => "Input (Press Esc to stop editing, Enter to send)",
        })
        .title_style(match app.input_mode {
            InputMode::Normal => Style::default(),
            InputMode::Editing => Style::default().fg(Color::Yellow),
        });

    let mut input_text = app.input.clone();
    if app.waiting_for_response {
        input_text = format!("Thinking{} (please wait)", app.thinking_dots);
    }

    let input_paragraph = Paragraph::new(input_text)
        .style(match app.input_mode {
            InputMode::Normal => Style::default(),
            InputMode::Editing => Style::default().fg(Color::Yellow),
        })
        .block(input_block)
        .wrap(Wrap { trim: true });
    f.render_widget(input_paragraph, area);

    // Set cursor position when editing
    if app.input_mode == InputMode::Editing && !app.waiting_for_response {
        f.set_cursor_position((
            area.x + app.input_cursor_position as u16 + 1,
            area.y + 1,
        ));
    }
}

/// Render the status bar
fn render_status_bar(f: &mut Frame, area: ratatui::layout::Rect, app: &App, args: &Args) {
    let status_text = format!(
        " Status: {} | Model: {} | Provider: {} | Messages: {} ",
        app.connection_status, args.model, args.provider, app.messages.len()
    );
    let status_paragraph = Paragraph::new(status_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_widget(status_paragraph, area);
}

/// Render the help popup
fn render_help_popup(f: &mut Frame, area: ratatui::layout::Rect) {
    let popup_area = centered_rect(60, 70, area);
    f.render_widget(Clear, popup_area);
    
    let help_text = vec![
        Line::from("🔧 th-chat Help"),
        Line::from(""),
        Line::from("Navigation:"),
        Line::from("  i          - Enter input mode"),
        Line::from("  Esc        - Exit input mode"),
        Line::from("  Enter      - Send message (in input mode)"),
        Line::from("  ↑/↓        - Scroll messages"),
        Line::from("  q          - Quit application"),
        Line::from("  F1         - Toggle this help"),
        Line::from(""),
        Line::from("Commands (type in input):"),
        Line::from("  /help      - Show help information"),
        Line::from("  /clear     - Clear conversation"),
        Line::from("  /debug     - Toggle debug mode"),
        Line::from("  /status    - Show connection status"),
        Line::from(""),
        Line::from("Press F1 or Esc to close this help"),
    ];

    let help_paragraph = Paragraph::new(help_text)
        .block(
            Block::default()
                .title("Help")
                .borders(Borders::ALL)
                .title_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: true });
    f.render_widget(help_paragraph, popup_area);
}

/// Helper function to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

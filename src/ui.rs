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
    if app.is_loading {
        render_loading_screen(f, app, args);
    } else {
        render_chat_screen(f, app, args);
    }
}

/// Render the Linux boot-style loading screen
pub fn render_loading_screen(f: &mut Frame, app: &App, _args: &Args) {
    let area = f.area();
    
    // Clear the entire screen with black background
    let background = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(background, area);
    
    // Create a simple layout - we want to start from the top-left like a real boot
    let main_area = ratatui::layout::Rect {
        x: 1,
        y: 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    
    // Boot header - show system info like real Linux boot
    let mut lines = vec![
        Line::from(vec![
            Span::styled("th-chat ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("v0.1.0", Style::default().fg(Color::White)),
            Span::styled(" starting up...", Style::default().fg(Color::Gray)),
        ]),
        Line::from(""),
    ];
    
    // Add each loading step with Linux-style formatting
    for (i, step) in app.loading_steps.iter().enumerate() {
        let is_current = i == app.current_step_index;
        
        // Create the line with proper spacing
        let status_symbol = step.status.symbol();
        let status_color = step.status.color();
        
        // For current step in progress, add blinking cursor effect
        let message = if is_current && matches!(step.status, crate::config::StepStatus::InProgress) {
            if app.boot_cursor_visible {
                format!("{}...", step.message)
            } else {
                format!("{}   ", step.message)
            }
        } else {
            step.message.clone()
        };
        
        let line = Line::from(vec![
            Span::styled(format!("{:<50}", message), Style::default().fg(Color::White)),
            Span::styled(status_symbol, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        ]);
        
        lines.push(line);
        
        // Add error details if step failed
        if let crate::config::StepStatus::Failed(error) = &step.status {
            lines.push(Line::from(vec![
                Span::styled("  Error: ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                Span::styled(error, Style::default().fg(Color::Red)),
            ]));
            lines.push(Line::from(""));
        }
    }
    
    // Add some spacing
    lines.push(Line::from(""));
    
    // Add system information like real Linux boot
    if app.is_loading_complete() {
        lines.push(Line::from(vec![
            Span::styled("System ready. Starting chat interface", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            if app.boot_cursor_visible { 
                Span::styled(".", Style::default().fg(Color::Green)) 
            } else { 
                Span::styled(" ", Style::default()) 
            }
        ]));
    } else if app.current_step_index < app.loading_steps.len() {
        // Show a kernel-style progress indicator
        let progress = (app.current_step_index as f32 / app.loading_steps.len() as f32 * 100.0) as u8;
        lines.push(Line::from(vec![
            Span::styled(format!("Progress: {}%", progress), Style::default().fg(Color::Yellow)),
        ]));
    }
    
    // Bottom status line - like Linux boot messages
    let footer_y = area.height.saturating_sub(3);
    let footer_area = ratatui::layout::Rect {
        x: 1,
        y: footer_y,
        width: area.width.saturating_sub(2),
        height: 1,
    };
    
    let footer_line = if app.loading_steps.iter().any(|s| matches!(s.status, crate::config::StepStatus::Failed(_))) {
        Line::from(vec![
            Span::styled("Boot failed. Press Ctrl+C to exit.", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        ])
    } else {
        Line::from(vec![
            Span::styled("Press Ctrl+C to abort startup", Style::default().fg(Color::DarkGray)),
        ])
    };
    
    // Render the main boot text
    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(Color::Black));
    f.render_widget(paragraph, main_area);
    
    // Render the footer
    let footer_paragraph = Paragraph::new(vec![footer_line])
        .style(Style::default().bg(Color::Black));
    f.render_widget(footer_paragraph, footer_area);
}

/// Render the main chat screen
pub fn render_chat_screen(f: &mut Frame, app: &mut App, args: &Args) {
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

/// Format a single MessageContent into displayable lines
fn format_message_content(content: &MessageContent, available_width: usize) -> Vec<Line<'static>> {
    match content {
        MessageContent::Text { text } => {
            let wrapped_text = textwrap::fill(text, available_width);
            wrapped_text
                .lines()
                .map(|line| Line::from(line.to_string()))
                .collect()
        }
        MessageContent::ToolUse { id, name, input } => {
            let mut lines = Vec::new();
            
            // Tool use header
            lines.push(Line::from(vec![
                Span::styled("🔧 ", Style::default().fg(Color::Magenta)),
                Span::styled("Tool Use: ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::styled(name.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            ]));
            
            // Tool ID (in a more subtle style)
            lines.push(Line::from(vec![
                Span::styled("   ID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(id.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            
            // Tool input (formatted JSON)
            let input_str = if input.is_null() {
                "No parameters".to_string()
            } else {
                match serde_json::to_string_pretty(input) {
                    Ok(formatted) => formatted,
                    Err(_) => format!("{}", input),
                }
            };
            
            lines.push(Line::from(vec![
                Span::styled("   Input: ", Style::default().fg(Color::Yellow)),
            ]));
            
            // Wrap and indent the input JSON
            let wrapped_input = textwrap::fill(&input_str, available_width.saturating_sub(6));
            for line in wrapped_input.lines() {
                lines.push(Line::from(vec![
                    Span::styled("     ", Style::default()),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
            
            lines
        }
        MessageContent::ToolResult { tool_use_id, content, is_error } => {
            let mut lines = Vec::new();
            
            // Tool result header
            let (icon, header_style) = if is_error.unwrap_or(false) {
                ("❌", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            } else {
                ("✅", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            };
            
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), header_style),
                Span::styled("Tool Result", header_style),
            ]));
            
            // Tool use ID reference
            lines.push(Line::from(vec![
                Span::styled("   For tool ID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(tool_use_id.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            
            // Tool result content
            for tool_content in content {
                match tool_content {
                    mcp_protocol::tool::ToolContent::Text { text } => {
                        lines.push(Line::from(vec![
                            Span::styled("   Output: ", Style::default().fg(Color::Cyan)),
                        ]));
                        
                        let wrapped_output = textwrap::fill(text, available_width.saturating_sub(6));
                        for line in wrapped_output.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("     ", Style::default()),
                                Span::styled(line.to_string(), Style::default().fg(Color::White)),
                            ]));
                        }
                    }
                    mcp_protocol::tool::ToolContent::Image { data, mime_type } => {
                        lines.push(Line::from(vec![
                            Span::styled("   📷 Image: ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                format!("{} ({} bytes)", mime_type, data.len()),
                                Style::default().fg(Color::White),
                            ),
                        ]));
                    }
                    mcp_protocol::tool::ToolContent::Audio { data, mime_type } => {
                        lines.push(Line::from(vec![
                            Span::styled("   🎵 Audio: ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                format!("{} ({} bytes)", mime_type, data.len()),
                                Style::default().fg(Color::White),
                            ),
                        ]));
                    }
                    mcp_protocol::tool::ToolContent::Resource { resource } => {
                        lines.push(Line::from(vec![
                            Span::styled("   🔗 Resource: ", Style::default().fg(Color::Cyan)),
                            Span::styled(
                                format!("{}", resource),
                                Style::default().fg(Color::White),
                            ),
                        ]));
                    }
                }
            }
            
            lines
        }
    }
}

/// Render the chat messages area with enhanced tool use support
fn render_chat_area(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let messages_block = Block::default()
        .borders(Borders::ALL)
        .title("Chat")
        .title_style(Style::default().fg(Color::Yellow));

    // Calculate available width for text wrapping (subtract borders and padding)
    let available_width = (area.width.saturating_sub(6)) as usize;
    
    // Flatten all messages into renderable items with proper line counting
    let mut all_items = Vec::new();
    
    for chat_msg in &app.messages {
        let message = chat_msg.as_message();
        let (prefix, role_style) = match message.role {
            Role::User => ("👤 You:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Role::Assistant => ("🤖 Assistant:", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
            Role::System => ("⚙️ System:", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        };
        
        // Add role header with completion info if available
        let header_text = if let Some(completion) = chat_msg.as_completion() {
            format!("{} [{}]", prefix, completion.model)
        } else {
            prefix.to_string()
        };
        all_items.push(ListItem::new(Line::from(Span::styled(header_text, role_style))));
        
        // Process each content item in the message
        for content in &message.content {
            let content_lines = format_message_content(content, available_width);
            for line in content_lines {
                all_items.push(ListItem::new(line));
            }
        }
        
        // Add token usage info for completions
        if let Some(completion) = chat_msg.as_completion() {
            let usage_text = format!(
                "📊 Tokens: {} in, {} out | Stop: {:?}",
                completion.usage.input_tokens,
                completion.usage.output_tokens,
                completion.stop_reason
            );
            all_items.push(ListItem::new(Line::from(Span::styled(
                usage_text,
                Style::default().fg(Color::DarkGray)
            ))));
        }
        
        // Add spacing between messages
        all_items.push(ListItem::new(Line::from("")));
    }

    let total_lines = all_items.len();
    let available_height = area.height.saturating_sub(2) as usize; // subtract borders
    
    // Calculate which items to show based on scroll
    let start_index = if total_lines <= available_height {
        // All content fits, no scrolling needed
        0
    } else {
        // Content is larger than screen, apply scrolling
        // vertical_scroll = 0 means show most recent (bottom)
        // vertical_scroll > 0 means scroll up to see older messages
        let max_scroll = total_lines.saturating_sub(available_height);
        max_scroll.saturating_sub(app.vertical_scroll)
    };
    
    let end_index = (start_index + available_height).min(total_lines);

    // Take the visible slice of items
    let visible_items: Vec<ListItem> = all_items
        .into_iter()
        .skip(start_index)
        .take(end_index - start_index)
        .collect();

    let messages_list = List::new(visible_items).block(messages_block);
    f.render_widget(messages_list, area);

    // Update scroll state for scrollbar
    if total_lines > available_height {
        app.scroll_state = app.scroll_state
            .content_length(total_lines)
            .position(start_index);
    }

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
        Line::from("Message Types:"),
        Line::from("  👤 User messages"),
        Line::from("  🤖 Assistant messages"),
        Line::from("  🔧 Tool use (function calls)"),
        Line::from("  ✅ Tool results (success)"),
        Line::from("  ❌ Tool results (error)"),
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
    f.render_widget(help_paragraph, area);
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

use genai_types::{messages::Role, MessageContent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, Wrap,
    },
    Frame,
};

use crate::app::{App, AppMode};
use crate::config::{CompatibleArgs, ToolDisplayMode};
use genai_types::Message;

/// Create a compact preview of tool input parameters
fn create_compact_input_preview(input: &serde_json::Value, max_length: usize) -> String {
    match input {
        serde_json::Value::Object(obj) => {
            let mut preview = String::new();
            let mut first = true;

            for (key, value) in obj.iter().take(3) {
                // Show max 3 params
                if !first {
                    preview.push_str(", ");
                }
                first = false;

                // Show key and abbreviated value
                preview.push_str(key);
                preview.push(':');

                let value_str = match value {
                    serde_json::Value::String(s) => {
                        if s.len() > 20 {
                            format!("\"{}...\"", &s[..17])
                        } else {
                            format!("\"{}\"", s)
                        }
                    }
                    serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
                    serde_json::Value::Object(obj) => format!("{{{} fields}}", obj.len()),
                    other => format!("{}", other),
                };

                preview.push_str(&value_str);

                if preview.len() > max_length {
                    break;
                }
            }

            if obj.len() > 3 {
                preview.push_str("...");
            }

            if preview.len() > max_length {
                preview.truncate(max_length.saturating_sub(3));
                preview.push_str("...");
            }

            preview
        }
        other => {
            let s = format!("{}", other);
            if s.len() > max_length {
                format!("{}...", &s[..max_length.saturating_sub(3)])
            } else {
                s
            }
        }
    }
}

/// Create a compact preview of tool output
fn create_compact_output_preview(
    content: &[mcp_protocol::tool::ToolContent],
    max_length: usize,
) -> String {
    if content.is_empty() {
        return String::new();
    }

    // Just preview the first text content for now
    for tool_content in content.iter().take(1) {
        match tool_content {
            mcp_protocol::tool::ToolContent::Text { text } => {
                let clean_text = text.trim().replace('\n', " ");
                if clean_text.len() > max_length {
                    return format!("{}...", &clean_text[..max_length.saturating_sub(3)]);
                } else {
                    return clean_text;
                }
            }
            mcp_protocol::tool::ToolContent::Image { mime_type, .. } => {
                return format!("Image ({})", mime_type);
            }
            mcp_protocol::tool::ToolContent::Audio { mime_type, .. } => {
                return format!("Audio ({})", mime_type);
            }
            mcp_protocol::tool::ToolContent::Resource { resource } => {
                return format!("Resource: {}", resource);
            }
        }
    }

    String::new()
}

/// Generate a preview text for a collapsed message
fn get_message_preview(message: &Message, max_length: usize) -> String {
    let mut preview = String::new();

    for content in &message.content {
        match content {
            MessageContent::Text { text } => {
                if preview.is_empty() {
                    preview = text.clone();
                } else {
                    preview.push_str(" ");
                    preview.push_str(text);
                }
            }
            MessageContent::ToolUse { name, .. } => {
                if !preview.is_empty() {
                    preview.push_str(" ");
                }
                preview.push_str(&format!("[{}]", name));
            }
            MessageContent::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                if !preview.is_empty() {
                    preview.push_str(" ");
                }
                let id_preview = if tool_use_id.len() >= 8 {
                    &tool_use_id[..8]
                } else {
                    tool_use_id
                };
                let status = if is_error.unwrap_or(false) {
                    "ERROR"
                } else {
                    "OK"
                };
                preview.push_str(&format!("[Tool Result {}: {}]", status, id_preview));
            }
        }

        // Stop if we're getting too long
        if preview.len() > max_length {
            break;
        }
    }

    // Truncate and add ellipsis if needed
    if preview.len() > max_length {
        preview.truncate(max_length.saturating_sub(3));
        preview.push_str("...");
    }

    if preview.is_empty() {
        "[Empty message]".to_string()
    } else {
        preview
    }
}

/// Render the main user interface
pub fn render(f: &mut Frame, app: &mut App, args: &CompatibleArgs) {
    if app.is_loading {
        render_loading_screen(f, app, args);
    } else {
        render_chat_screen(f, app, args);
    }
}

/// Render the Linux boot-style loading screen
pub fn render_loading_screen(f: &mut Frame, app: &App, _args: &CompatibleArgs) {
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
            Span::styled(
                "th-chat ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
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
        let message = if is_current && matches!(step.status, crate::config::StepStatus::InProgress)
        {
            if app.boot_cursor_visible {
                format!("{}...", step.message)
            } else {
                format!("{}   ", step.message)
            }
        } else {
            step.message.clone()
        };

        let line = Line::from(vec![
            Span::styled(
                format!("{:<50}", message),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                status_symbol,
                Style::default()
                    .fg(status_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        lines.push(line);

        // Add error details if step failed
        if let crate::config::StepStatus::Failed(error) = &step.status {
            lines.push(Line::from(vec![
                Span::styled(
                    "  Error: ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
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
            Span::styled(
                "System ready. Starting chat interface",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            if app.boot_cursor_visible {
                Span::styled(".", Style::default().fg(Color::Green))
            } else {
                Span::styled(" ", Style::default())
            },
        ]));
    } else if app.current_step_index < app.loading_steps.len() {
        // Show a kernel-style progress indicator
        let progress =
            (app.current_step_index as f32 / app.loading_steps.len() as f32 * 100.0) as u8;
        lines.push(Line::from(vec![Span::styled(
            format!("Progress: {}%", progress),
            Style::default().fg(Color::Yellow),
        )]));
    }

    // Bottom status line - like Linux boot messages
    let footer_y = area.height.saturating_sub(3);
    let footer_area = ratatui::layout::Rect {
        x: 1,
        y: footer_y,
        width: area.width.saturating_sub(2),
        height: 1,
    };

    let footer_line = if app
        .loading_steps
        .iter()
        .any(|s| matches!(s.status, crate::config::StepStatus::Failed(_)))
    {
        Line::from(vec![Span::styled(
            "Boot failed. Press Ctrl+C to exit.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )])
    } else {
        Line::from(vec![Span::styled(
            "Press Ctrl+C to abort startup",
            Style::default().fg(Color::DarkGray),
        )])
    };

    // Render the main boot text
    let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Black));
    f.render_widget(paragraph, main_area);

    // Render the footer
    let footer_paragraph =
        Paragraph::new(vec![footer_line]).style(Style::default().bg(Color::Black));
    f.render_widget(footer_paragraph, footer_area);
}

/// Render the main chat screen
pub fn render_chat_screen(f: &mut Frame, app: &mut App, args: &CompatibleArgs) {
    let size = f.area();

    if app.show_split_screen {
        // Split screen mode - chat on left, help panel on right
        render_split_screen_layout(f, size, app, args);
    } else {
        // Full screen mode - original layout
        render_full_screen_layout(f, size, app, args);
    }

    // Full-screen help popup (rendered on top if active) - overrides everything
    if app.show_help {
        render_help_popup(f, size);
    }
}

/// Render the split screen layout
fn render_split_screen_layout(f: &mut Frame, size: ratatui::layout::Rect, app: &mut App, args: &CompatibleArgs) {
    // Create horizontal split - left side for chat, right side for help panel
    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50), // Left side - chat interface
            Constraint::Percentage(50), // Right side - help panel
        ])
        .split(size);

    // Calculate available width for input wrapping (based on left panel width)
    let input_available_width = (horizontal_chunks[0].width.saturating_sub(4)) as usize;

    // Update cursor position calculation
    app.calculate_cursor_position(input_available_width);

    // Calculate input area height dynamically
    let input_height = app.get_input_height();

    // Create main layout for left side (chat interface)
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // Title bar
            Constraint::Min(1),               // Chat area (takes remaining space)
            Constraint::Length(input_height), // Input area (flexible)
            Constraint::Length(1),            // Status bar
        ])
        .split(horizontal_chunks[0]);

    // Title bar (left side)
    render_title_bar(f, left_chunks[0], args);

    // Chat messages area (left side)
    render_chat_area(f, left_chunks[1], app);

    // Input area (left side)
    render_flexible_input_area(f, left_chunks[2], app);

    // Status bar (left side)
    render_status_bar(f, left_chunks[3], app, args);

    // Right side - help panel
    render_help_panel(f, horizontal_chunks[1], app);
}

/// Render the full screen layout (original)
fn render_full_screen_layout(f: &mut Frame, size: ratatui::layout::Rect, app: &mut App, args: &CompatibleArgs) {
    // Calculate available width for input wrapping (full width)
    let input_available_width = (size.width.saturating_sub(4)) as usize;

    // Update cursor position calculation
    app.calculate_cursor_position(input_available_width);

    // Calculate input area height dynamically
    let input_height = app.get_input_height();

    // Create main layout with flexible input area
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),            // Title bar
            Constraint::Min(1),               // Chat area (takes remaining space)
            Constraint::Length(input_height), // Input area (flexible)
            Constraint::Length(1),            // Status bar
        ])
        .split(size);

    // Title bar
    render_title_bar(f, chunks[0], args);

    // Chat messages area
    render_chat_area(f, chunks[1], app);

    // Input area
    render_flexible_input_area(f, chunks[2], app);

    // Status bar
    render_status_bar(f, chunks[3], app, args);
}

/// Render the title bar
fn render_title_bar(f: &mut Frame, area: ratatui::layout::Rect, args: &CompatibleArgs) {
    let title = format!("th-chat - {}", args.title);
    let title_paragraph = Paragraph::new(title)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    f.render_widget(title_paragraph, area);
}

/// Format a single MessageContent into displayable lines
fn format_message_content(
    content: &MessageContent,
    available_width: usize,
    tool_display_mode: &ToolDisplayMode,
) -> Vec<Line<'static>> {
    match content {
        MessageContent::Text { text } => {
            let wrapped_text = textwrap::fill(text, available_width);
            wrapped_text
                .lines()
                .map(|line| Line::from(line.to_string()))
                .collect()
        }
        MessageContent::ToolUse { id, name, input } => {
            match tool_display_mode {
                ToolDisplayMode::Minimal => {
                    vec![Line::from(vec![Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )])]
                }
                ToolDisplayMode::Compact => {
                    let mut lines = vec![Line::from(vec![Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )])];

                    if !input.is_null() && !input.as_object().map_or(false, |obj| obj.is_empty()) {
                        let input_preview =
                            create_compact_input_preview(input, available_width.saturating_sub(20));
                        if !input_preview.is_empty() {
                            lines.push(Line::from(vec![
                                Span::styled("   ".to_string(), Style::default()),
                                Span::styled(
                                    "→ ".to_string(),
                                    Style::default().fg(Color::DarkGray),
                                ),
                                Span::styled(input_preview, Style::default().fg(Color::Gray)),
                            ]));
                        }
                    }
                    lines
                }
                ToolDisplayMode::Full => {
                    // Original full display
                    let mut lines = Vec::new();

                    lines.push(Line::from(vec![
                        Span::styled(
                            "Tool Use: ".to_string(),
                            Style::default()
                                .fg(Color::Magenta)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            name.clone(),
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));

                    lines.push(Line::from(vec![
                        Span::styled("   ID: ".to_string(), Style::default().fg(Color::DarkGray)),
                        Span::styled(id.clone(), Style::default().fg(Color::DarkGray)),
                    ]));

                    let input_str = if input.is_null() {
                        "No parameters".to_string()
                    } else {
                        match serde_json::to_string_pretty(input) {
                            Ok(formatted) => formatted,
                            Err(_) => format!("{}", input),
                        }
                    };

                    lines.push(Line::from(vec![Span::styled(
                        "   Input: ".to_string(),
                        Style::default().fg(Color::Yellow),
                    )]));

                    let wrapped_input =
                        textwrap::fill(&input_str, available_width.saturating_sub(6));
                    for line in wrapped_input.lines() {
                        lines.push(Line::from(vec![
                            Span::styled("     ".to_string(), Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(Color::White)),
                        ]));
                    }

                    lines
                }
            }
        }
        MessageContent::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let is_error = is_error.unwrap_or(false);
            match tool_display_mode {
                ToolDisplayMode::Minimal => {
                    let (symbol, color) = if is_error {
                        ("[ERROR]", Color::Red)
                    } else {
                        ("[✓]", Color::Green)
                    };

                    vec![Line::from(vec![Span::styled(
                        symbol.to_string(),
                        Style::default().fg(color),
                    )])]
                }
                ToolDisplayMode::Compact => {
                    let (symbol, _status_text, color) = if is_error {
                        ("[ERROR]", "", Color::Red)
                    } else {
                        ("[✓]", "", Color::Green)
                    };

                    let mut lines = vec![Line::from(vec![Span::styled(
                        symbol.to_string(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )])];

                    let output_preview =
                        create_compact_output_preview(content, available_width.saturating_sub(20));
                    if !output_preview.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("   ".to_string(), Style::default()),
                            Span::styled("← ".to_string(), Style::default().fg(Color::DarkGray)),
                            Span::styled(output_preview, Style::default().fg(Color::Gray)),
                        ]));
                    }
                    lines
                }
                ToolDisplayMode::Full => {
                    // Original full display
                    let mut lines = Vec::new();

                    let (prefix, header_style) = if is_error {
                        (
                            "Tool Result [ERROR]",
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        )
                    } else {
                        (
                            "Tool Result [✓]",
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        )
                    };

                    lines.push(Line::from(vec![Span::styled(
                        prefix.to_string(),
                        header_style,
                    )]));

                    lines.push(Line::from(vec![
                        Span::styled(
                            "   For tool ID: ".to_string(),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(tool_use_id.clone(), Style::default().fg(Color::DarkGray)),
                    ]));

                    for tool_content in content {
                        match tool_content {
                            mcp_protocol::tool::ToolContent::Text { text } => {
                                lines.push(Line::from(vec![Span::styled(
                                    "   Output: ".to_string(),
                                    Style::default().fg(Color::Cyan),
                                )]));

                                let wrapped_output =
                                    textwrap::fill(text, available_width.saturating_sub(6));
                                for line in wrapped_output.lines() {
                                    lines.push(Line::from(vec![
                                        Span::styled("     ".to_string(), Style::default()),
                                        Span::styled(
                                            line.to_string(),
                                            Style::default().fg(Color::White),
                                        ),
                                    ]));
                                }
                            }
                            mcp_protocol::tool::ToolContent::Image { data, mime_type } => {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "   Image: ".to_string(),
                                        Style::default().fg(Color::Cyan),
                                    ),
                                    Span::styled(
                                        format!("{} ({} bytes)", mime_type, data.len()),
                                        Style::default().fg(Color::White),
                                    ),
                                ]));
                            }
                            mcp_protocol::tool::ToolContent::Audio { data, mime_type } => {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "   Audio: ".to_string(),
                                        Style::default().fg(Color::Cyan),
                                    ),
                                    Span::styled(
                                        format!("{} ({} bytes)", mime_type, data.len()),
                                        Style::default().fg(Color::White),
                                    ),
                                ]));
                            }
                            mcp_protocol::tool::ToolContent::Resource { resource } => {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "   Resource: ".to_string(),
                                        Style::default().fg(Color::Cyan),
                                    ),
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
    }
}

/// Render the chat messages area with enhanced tool use support and message navigation
fn render_chat_area(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let messages_block = Block::default().borders(Borders::ALL);

    // Calculate available width for text wrapping (subtract borders and padding)
    let available_width = (area.width.saturating_sub(6)) as usize;

    // Flatten all messages into renderable items with proper line counting
    let mut all_items = Vec::new();
    let selected_message_index = app.get_selected_message_index();

    for (msg_index, chat_msg) in app.messages.iter().enumerate() {
        let message = chat_msg.as_message();
        let mut role_style = match message.role {
            Role::User => Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Role::Assistant => Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            Role::System => Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        };

        // Highlight selected message
        let is_selected = selected_message_index == Some(msg_index);
        if is_selected {
            role_style = role_style.bg(Color::White).fg(Color::Black);
        }

        // Create a simple header without role prefix or model info
        let header_text = "".to_string();

        // Add message selection indicator (only for non-empty headers)
        if !header_text.is_empty() {
            let header_with_indicator = if is_selected {
                format!("► {}", header_text)
            } else {
                format!("  {}", header_text)
            };

            all_items.push(ListItem::new(Line::from(Span::styled(
                header_with_indicator,
                role_style,
            ))));
        }

        // Process each content item in the message
        let is_collapsed = app.is_message_collapsed(msg_index);

        if is_collapsed {
            // Show collapsed message as a single line with preview and left border
            let preview_text = get_message_preview(&message, 60);
            let border_char = match message.role {
                Role::User => "│",
                Role::Assistant => "│",
                Role::System => "│",
            };
            let border_color = match message.role {
                Role::User => Color::Green,
                Role::Assistant => Color::Blue,
                Role::System => Color::Yellow,
            };

            let collapsed_line = if is_selected {
                Line::from(vec![
                    Span::styled(
                        border_char,
                        Style::default().fg(border_color).bg(Color::DarkGray),
                    ),
                    Span::styled(
                        " [COLLAPSED] ",
                        Style::default().fg(Color::DarkGray).bg(Color::DarkGray),
                    ),
                    Span::styled(
                        preview_text,
                        Style::default().fg(Color::DarkGray).bg(Color::DarkGray),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(border_char, Style::default().fg(border_color)),
                    Span::styled(" [COLLAPSED] ", Style::default().fg(Color::DarkGray)),
                    Span::styled(preview_text, Style::default().fg(Color::Gray)),
                ])
            };
            all_items.push(ListItem::new(collapsed_line));
        } else {
            // Show full message content with left border
            let border_char = match message.role {
                Role::User => "│",
                Role::Assistant => "│",
                Role::System => "│",
            };
            let border_color = match message.role {
                Role::User => Color::Green,
                Role::Assistant => Color::Blue,
                Role::System => Color::Yellow,
            };

            for content in &message.content {
                let content_lines =
                    format_message_content(content, available_width, &app.tool_display_mode);
                for line in content_lines {
                    // Apply background highlighting to selected message content
                    let styled_line = if is_selected {
                        Line::from(
                            vec![
                                Span::styled(
                                    border_char,
                                    Style::default().fg(border_color).bg(Color::DarkGray),
                                ),
                                Span::styled(" ", Style::default().bg(Color::DarkGray)),
                            ]
                            .into_iter()
                            .chain(line.spans.into_iter().map(|span| {
                                Span::styled(span.content, span.style.bg(Color::DarkGray))
                            }))
                            .collect::<Vec<_>>(),
                        )
                    } else {
                        Line::from(
                            vec![
                                Span::styled(border_char, Style::default().fg(border_color)),
                                Span::styled(" ", Style::default()),
                            ]
                            .into_iter()
                            .chain(line.spans.into_iter().map(|span| span))
                            .collect::<Vec<_>>(),
                        )
                    };
                    all_items.push(ListItem::new(styled_line));
                }
            }
        }

        // Add model and token usage info for completions (only for non-collapsed messages)
        if !is_collapsed {
            if let Some(completion) = chat_msg.as_completion() {
                let usage_text = format!(
                    "{} | Tokens: {} in, {} out | Stop: {:?}",
                    completion.model,
                    completion.usage.input_tokens,
                    completion.usage.output_tokens,
                    completion.stop_reason
                );
                let usage_style = if is_selected {
                    Style::default().fg(Color::DarkGray).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                // Define border char and color for metadata line
                let border_char = match message.role {
                    Role::User => "│",
                    Role::Assistant => "│",
                    Role::System => "│",
                };
                let border_color = match message.role {
                    Role::User => Color::Green,
                    Role::Assistant => Color::Blue,
                    Role::System => Color::Yellow,
                };
                let border_style = if is_selected {
                    Style::default().fg(border_color).bg(Color::DarkGray)
                } else {
                    Style::default().fg(border_color)
                };

                all_items.push(ListItem::new(Line::from(vec![
                    Span::styled(border_char, border_style),
                    Span::styled(" ", usage_style),
                    Span::styled(usage_text, usage_style),
                ])));
            }
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
        app.scroll_state = app
            .scroll_state
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

/// Render the flexible input area that expands with content
fn render_flexible_input_area(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let input_block = Block::default().borders(Borders::ALL);

    let mut input_text = app.input.clone();
    if app.waiting_for_response {
        input_text = format!("Thinking{} (please wait)", app.thinking_dots);
    }

    let input_paragraph = Paragraph::new(input_text)
        .style(match app.app_mode {
            AppMode::Input => Style::default().fg(Color::Yellow),
            _ => Style::default(),
        })
        .block(input_block)
        .wrap(Wrap { trim: false }); // Don't trim for proper multi-line handling

    f.render_widget(input_paragraph, area);

    // Set cursor position when editing (accounting for multi-line)
    if app.app_mode.is_input() && !app.waiting_for_response {
        f.set_cursor_position((
            area.x + app.cursor_col as u16 + 1,
            area.y + app.cursor_line as u16 + 1,
        ));
    }
}

/// Render the status bar
fn render_status_bar(f: &mut Frame, area: ratatui::layout::Rect, app: &App, args: &CompatibleArgs) {
    let mode_text = match app.app_mode {
        AppMode::Input => "INPUT",
        AppMode::View => "VIEW",
        AppMode::Chat => "CHAT",
    };

    let mode_color = match app.app_mode {
        AppMode::Input => Color::Yellow,
        AppMode::View => Color::Blue,
        AppMode::Chat => Color::Magenta,
    };

    // Create spans with different colors for the mode
    let status_base = format!(
        " Status: {} | Model: {} | Provider: {} | Messages: {} | Mode: ",
        app.connection_status,
        args.model,
        args.provider,
        app.messages.len()
    );

    let tool_mode = format!(" | Tools: {}", app.tool_display_mode.display_name());
    let split_screen_mode = format!(" | Panel: {}", if app.show_split_screen { "Split" } else { "Full" });

    let status_line = Line::from(vec![
        Span::styled(status_base, Style::default().fg(Color::White)),
        Span::styled(
            mode_text,
            Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(tool_mode, Style::default().fg(Color::Cyan)),
        Span::styled(split_screen_mode, Style::default().fg(Color::Green)),
    ]);

    let status_paragraph =
        Paragraph::new(vec![status_line]).style(Style::default().bg(Color::DarkGray));
    f.render_widget(status_paragraph, area);
}

/// Render the enhanced help popup with navigation instructions
fn render_help_popup(f: &mut Frame, area: ratatui::layout::Rect) {
    let popup_area = centered_rect(80, 90, area);
    f.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "th-chat Help",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Application Modes:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "View Mode (default):",
            Style::default().fg(Color::Blue),
        )]),
        Line::from("  j / k / ↓ / ↑ - Scroll through conversation"),
        Line::from("  i           - Enter Input mode to compose messages"),
        Line::from("  v           - Enter Chat mode for message operations"),
        Line::from("  Enter       - Send current input (if any)"),
        Line::from("  t           - Cycle tool display mode"),
        Line::from("  T           - Auto-collapse tool-heavy messages"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Input Mode:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Esc         - Return to View mode"),
        Line::from("  Enter       - Insert newline"),
        Line::from("  Ctrl+Enter  - Send message"),
        Line::from("  ↑/↓         - Navigate between lines"),
        Line::from("  Home/End    - Move to start/end of line"),
        Line::from("  Ctrl+A      - Move to start of input"),
        Line::from("  Ctrl+E      - Move to end of input"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Chat Mode:",
            Style::default().fg(Color::Magenta),
        )]),
        Line::from("  Esc         - Return to View mode"),
        Line::from("  j / k / ↓ / ↑ - Navigate between messages"),
        Line::from("  c           - Toggle collapse/expand selected message"),
        Line::from("  t           - Cycle tool display mode"),
        Line::from("  T           - Auto-collapse tool-heavy messages"),
        Line::from("  Selected message shows with ► indicator and highlighting"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Tool Display:",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  Minimal    - Just show tool names and status symbols"),
        Line::from("  Compact    - Show tool names with input/output previews"),
        Line::from("  Full       - Show complete tool details (traditional)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "General:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  q          - Quit application"),
        Line::from("  h / F1     - Toggle this help"),
        Line::from("  s          - Toggle split screen (help panel)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Commands:",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from("  /help /clear /debug /status  (type in input area)"),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Tips:",
            Style::default().fg(Color::Cyan),
        )]),
        Line::from("  Input area expands automatically with content"),
        Line::from("  Use Ctrl+Enter to send multi-line messages"),
        Line::from("  Mode shown in status bar: INPUT | VIEW | CHAT"),
        Line::from(""),
        Line::from("Press h/F1 or Esc to close this help"),
    ];

    let help_paragraph = Paragraph::new(help_text)
        .block(
            Block::default()
                .title("Help - Multi-line Input Support")
                .borders(Borders::ALL)
                .title_style(Style::default().fg(Color::Yellow))
                .style(Style::default().bg(Color::Black).fg(Color::White)),
        )
        .style(Style::default().bg(Color::Black).fg(Color::White))
        .wrap(Wrap { trim: true });
    f.render_widget(help_paragraph, popup_area);
}

/// Render the help panel on the right side
fn render_help_panel(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let help_block = Block::default()
        .title("Available Commands")
        .borders(Borders::ALL)
        .title_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(Color::Black));

    // Generate help content based on current mode
    let help_content = get_mode_specific_help(app);

    let help_paragraph = Paragraph::new(help_content)
        .block(help_block)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: true });

    f.render_widget(help_paragraph, area);
}

/// Get mode-specific help content
fn get_mode_specific_help(app: &App) -> Vec<Line> {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            format!("Current Mode: {}", match app.app_mode {
                AppMode::Input => "INPUT",
                AppMode::View => "VIEW", 
                AppMode::Chat => "CHAT",
            }),
            Style::default()
                .fg(match app.app_mode {
                    AppMode::Input => Color::Yellow,
                    AppMode::View => Color::Blue,
                    AppMode::Chat => Color::Magenta,
                })
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
    ];

    match app.app_mode {
        AppMode::View => {
            lines.extend(vec![
                Line::from(vec![Span::styled(
                    "VIEW MODE COMMANDS:",
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from("j/k/↓/↑ - Scroll messages"),
                Line::from("i - Enter INPUT mode"),
                Line::from("v - Enter CHAT mode"),
                Line::from("Enter - Send current input"),
                Line::from("t - Cycle tool display"),
                Line::from("T - Auto-collapse tools"),
                Line::from("h/F1 - Toggle full help"),
                Line::from("s - Toggle split screen"),
                Line::from("q - Quit application"),
            ]);
        }
        AppMode::Input => {
            lines.extend(vec![
                Line::from(vec![Span::styled(
                    "INPUT MODE COMMANDS:",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from("Esc - Return to VIEW mode"),
                Line::from("Enter - Insert newline"),
                Line::from("Ctrl+Enter - Send message"),
                Line::from("↑/↓ - Navigate lines"),
                Line::from("Home/End - Line start/end"),
                Line::from("Ctrl+A - Input start"),
                Line::from("Ctrl+E - Input end"),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "SPECIAL COMMANDS:",
                    Style::default().fg(Color::Cyan),
                )]),
                Line::from("/help - Show commands"),
                Line::from("/clear - Clear screen"),
                Line::from("/debug - Debug info"),
                Line::from("/status - Connection status"),
            ]);
        }
        AppMode::Chat => {
            lines.extend(vec![
                Line::from(vec![Span::styled(
                    "CHAT MODE COMMANDS:",
                    Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                )]),
                Line::from(""),
                Line::from("Esc - Return to VIEW mode"),
                Line::from("j/k/↓/↑ - Navigate messages"),
                Line::from("c - Toggle collapse message"),
                Line::from("t - Cycle tool display"),
                Line::from("T - Auto-collapse tools"),
                Line::from(""),
                Line::from(vec![Span::styled(
                    "MESSAGE SELECTION:",
                    Style::default().fg(Color::Cyan),
                )]),
                Line::from("► - Selected message indicator"),
                Line::from("Background highlighting shown"),
            ]);
        }
    }

    lines.push(Line::from(""));
    lines.extend(vec![
        Line::from(vec![Span::styled(
            "TOOL DISPLAY MODES:",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Current: ", Style::default().fg(Color::White)),
            Span::styled(
                app.tool_display_mode.display_name(),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from("• Minimal - Names only"),
        Line::from("• Compact - With previews"),
        Line::from("• Full - Complete details"),
    ]);

    lines.push(Line::from(""));
    lines.extend(vec![
        Line::from(vec![Span::styled(
            "CONNECTION INFO:",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::White)),
            Span::styled(
                &app.connection_status,
                if app.connection_status == "Connected" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Red)
                },
            ),
        ]),
        Line::from(vec![
            Span::styled("Messages: ", Style::default().fg(Color::White)),
            Span::styled(
                app.messages.len().to_string(),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ]);

    if app.waiting_for_response {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            format!("Thinking{}", app.thinking_dots),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Press 's' to toggle this panel", Style::default().fg(Color::DarkGray)),
    ]));

    lines
}

/// Helper function to create a centered rect
fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
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

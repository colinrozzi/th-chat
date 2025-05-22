use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use genai_types::{messages::Role, Message, MessageContent};
use ratatui::{backend::Backend, Terminal};
use std::time::{Duration, Instant};

use crate::chat::{ChatManager, ChatMessage};
use crate::config::Args;
use crate::ui;

/// Current input mode
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    Editing,
}

/// Application state
#[derive(Debug)]
pub struct App {
    /// Current input mode
    pub input_mode: InputMode,
    /// Current value of the input box
    pub input: String,
    /// Current input cursor position
    pub input_cursor_position: usize,
    /// History of recorded messages
    pub messages: Vec<ChatMessage>,
    /// Current position in the message list
    pub messages_state: usize,
    /// Whether we should quit
    pub should_quit: bool,
    /// Connection status
    pub connection_status: String,
    /// Whether we're currently waiting for a response
    pub waiting_for_response: bool,
    /// Thinking animation state
    pub thinking_dots: String,
    /// Last thinking update time
    pub last_thinking_update: Instant,
    /// Debug mode
    pub debug: bool,
    /// Show help popup
    pub show_help: bool,
    /// Scroll state for messages
    pub scroll_state: ratatui::widgets::ScrollbarState,
    /// Vertical scroll offset
    pub vertical_scroll: usize,
}

impl Default for App {
    fn default() -> App {
        App {
            input_mode: InputMode::Normal,
            input: String::new(),
            input_cursor_position: 0,
            messages: Vec::new(),
            messages_state: 0,
            should_quit: false,
            connection_status: "Disconnected".to_string(),
            waiting_for_response: false,
            thinking_dots: ".".to_string(),
            last_thinking_update: Instant::now(),
            debug: false,
            show_help: false,
            scroll_state: ratatui::widgets::ScrollbarState::default(),
            vertical_scroll: 0,
        }
    }
}

impl App {
    pub fn new(debug: bool) -> Self {
        App {
            debug,
            ..Default::default()
        }
    }

    /// Main application loop
    pub async fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        chat_manager: &mut ChatManager,
        args: &Args,
    ) -> Result<()> {
        // Main event loop
        loop {
            self.update_thinking_animation();
            
            terminal.draw(|f| ui::render(f, self, args))?;

            if self.should_quit {
                break;
            }

            // Handle events with timeout for animation updates
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key_event) => {
                        if let Some(message) = self.handle_key_event(key_event)? {
                            self.handle_message(message, chat_manager, args).await?;
                        }
                    }
                    Event::Resize(_, _) => {
                        // Handle terminal resize
                    }
                    _ => {}
                }
            }
        }

        // Cleanup
        chat_manager.cleanup().await?;
        Ok(())
    }

    /// Handle a key event and return a message if one should be sent
    fn handle_key_event(
        &mut self,
        key_event: crossterm::event::KeyEvent,
    ) -> Result<Option<String>> {
        if key_event.kind != KeyEventKind::Press {
            return Ok(None);
        }

        // Handle help popup
        if self.show_help {
            match key_event.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(1) => {
                    self.toggle_help();
                }
                _ => {}
            }
            return Ok(None);
        }

        match self.input_mode {
            InputMode::Normal => match key_event.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                KeyCode::Char('i') => {
                    self.input_mode = InputMode::Editing;
                }
                KeyCode::Up => {
                    self.scroll_up();
                }
                KeyCode::Down => {
                    self.scroll_down();
                }
                KeyCode::F(1) => {
                    self.toggle_help();
                }
                _ => {}
            },
            InputMode::Editing => match key_event.code {
                KeyCode::Enter => {
                    if let Some(message) = self.submit_message() {
                        self.input_mode = InputMode::Normal;
                        return Ok(Some(message));
                    }
                }
                KeyCode::Char(to_insert) => {
                    self.enter_char(to_insert);
                }
                KeyCode::Backspace => {
                    self.delete_char();
                }
                KeyCode::Left => {
                    self.move_cursor_left();
                }
                KeyCode::Right => {
                    self.move_cursor_right();
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
        }
        Ok(None)
    }

    /// Handle a message that was submitted by the user
    async fn handle_message(
        &mut self,
        message: String,
        chat_manager: &mut ChatManager,
        args: &Args,
    ) -> Result<()> {
        // Handle special commands
        if message.starts_with('/') {
            self.handle_command(&message);
            return Ok(());
        }

        // Add user message to display
        let user_message = ChatMessage {
            id: None,
            parent_id: None,
            message: Message {
                role: Role::User,
                content: vec![MessageContent::Text { text: message.clone() }],
            },
        };
        self.add_message(user_message);

        // Send message to chat manager and get response
        self.set_waiting(true);
        
        match chat_manager.send_message(message).await {
            Ok(response_messages) => {
                self.set_waiting(false);
                for msg in response_messages {
                    self.add_message(msg);
                }
            }
            Err(e) => {
                self.set_waiting(false);
                let error_msg = format!("Error: {}", e);
                let system_message = ChatMessage {
                    id: None,
                    parent_id: None,
                    message: Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: error_msg }],
                    },
                };
                self.add_message(system_message);
            }
        }

        Ok(())
    }

    /// Handle special commands
    fn handle_command(&mut self, command: &str) {
        match command {
            "/help" => {
                self.toggle_help();
            }
            "/clear" => {
                self.messages.clear();
                self.update_scroll();
            }
            "/debug" => {
                self.debug = !self.debug;
                let status_msg = if self.debug { "Debug mode enabled" } else { "Debug mode disabled" };
                let system_message = ChatMessage {
                    id: None,
                    parent_id: None,
                    message: Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: status_msg.to_string() }],
                    },
                };
                self.add_message(system_message);
            }
            "/status" => {
                let status_text = format!(
                    "Connection: {} | Messages: {}",
                    self.connection_status,
                    self.messages.len()
                );
                let system_message = ChatMessage {
                    id: None,
                    parent_id: None,
                    message: Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: status_text }],
                    },
                };
                self.add_message(system_message);
            }
            _ => {
                let error_msg = format!("Unknown command: {}. Type /help for available commands.", command);
                let system_message = ChatMessage {
                    id: None,
                    parent_id: None,
                    message: Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: error_msg }],
                    },
                };
                self.add_message(system_message);
            }
        }
    }

    /// Move cursor left in input
    pub fn move_cursor_left(&mut self) {
        let cursor_moved_left = self.input_cursor_position.saturating_sub(1);
        self.input_cursor_position = self.clamp_cursor(cursor_moved_left);
    }

    /// Move cursor right in input
    pub fn move_cursor_right(&mut self) {
        let cursor_moved_right = self.input_cursor_position.saturating_add(1);
        self.input_cursor_position = self.clamp_cursor(cursor_moved_right);
    }

    /// Enter a character into input at cursor position
    pub fn enter_char(&mut self, new_char: char) {
        let index = self.byte_index();
        self.input.insert(index, new_char);
        self.move_cursor_right();
    }

    /// Get the byte index for the cursor position
    fn byte_index(&self) -> usize {
        self.input
            .char_indices()
            .map(|(i, _)| i)
            .nth(self.input_cursor_position)
            .unwrap_or(self.input.len())
    }

    /// Delete character at cursor
    pub fn delete_char(&mut self) {
        let is_not_cursor_leftmost = self.input_cursor_position != 0;
        if is_not_cursor_leftmost {
            let current_index = self.input_cursor_position;
            let from_left_to_current_index = current_index - 1;
            let before_char_to_delete = self.input.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.input.chars().skip(current_index);
            self.input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();
        }
    }

    /// Clamp cursor position to input length
    fn clamp_cursor(&self, new_cursor_pos: usize) -> usize {
        new_cursor_pos.clamp(0, self.input.chars().count())
    }

    /// Reset input field
    pub fn reset_input(&mut self) {
        self.input.clear();
        self.input_cursor_position = 0;
    }

    /// Submit current input
    pub fn submit_message(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            return None;
        }
        let message = self.input.clone();
        self.reset_input();
        Some(message)
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.messages_state = self.messages.len().saturating_sub(1);
        self.update_scroll();
        // Auto-scroll to bottom
        self.vertical_scroll = self.messages.len().saturating_sub(1);
        self.scroll_state = self.scroll_state.position(self.vertical_scroll);
    }

    /// Update scroll state based on messages
    pub fn update_scroll(&mut self) {
        self.scroll_state = self.scroll_state.content_length(self.messages.len());
    }

    /// Scroll messages up
    pub fn scroll_up(&mut self) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(1);
        self.scroll_state = self.scroll_state.position(self.vertical_scroll);
    }

    /// Scroll messages down
    pub fn scroll_down(&mut self) {
        if self.vertical_scroll < self.messages.len().saturating_sub(1) {
            self.vertical_scroll += 1;
        }
        self.scroll_state = self.scroll_state.position(self.vertical_scroll);
    }

    /// Update thinking animation
    pub fn update_thinking_animation(&mut self) {
        if self.waiting_for_response && self.last_thinking_update.elapsed() > Duration::from_millis(500) {
            self.thinking_dots = match self.thinking_dots.as_str() {
                "." => "..".to_string(),
                ".." => "...".to_string(),
                "..." => ".".to_string(),
                _ => ".".to_string(),
            };
            self.last_thinking_update = Instant::now();
        }
    }

    /// Set waiting for response state
    pub fn set_waiting(&mut self, waiting: bool) {
        self.waiting_for_response = waiting;
        if waiting {
            self.thinking_dots = ".".to_string();
            self.last_thinking_update = Instant::now();
        }
    }

    /// Toggle help popup
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Set connection status
    pub fn set_connection_status(&mut self, status: String) {
        self.connection_status = status;
    }
}

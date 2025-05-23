use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use genai_types::{messages::Role, Message, MessageContent};
use ratatui::{backend::Backend, Terminal};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, debug, error};
use uuid::Uuid;

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
    /// History of recorded messages (for UI compatibility)
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
    /// Client's current head in the conversation chain
    pub client_head: Option<String>,
    /// Messages indexed by their ID for efficient lookup
    pub messages_by_id: HashMap<String, ChatMessage>,
    /// Ordered list of message IDs representing the current conversation view
    pub message_chain: Vec<String>,
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
            client_head: None,
            messages_by_id: HashMap::new(),
            message_chain: Vec::new(),
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

    /// Main application loop with chain synchronization
    pub async fn run<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        chat_manager: &mut ChatManager,
        args: &Args,
    ) -> Result<()> {
        // Initial sync to load any existing conversation
        info!("Performing initial sync with chat-state actor");
        self.sync_with_chat_state(chat_manager).await
            .unwrap_or_else(|e| {
                error!("Failed initial sync: {:?}", e);
            });
        
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
                            // Handle special commands
                            if message.starts_with('/') {
                                self.handle_command(&message[1..]);
                            } else {
                                // Send message and sync
                                self.waiting_for_response = true;
                                
                                match chat_manager.send_message_get_head(message).await {
                                    Ok(_new_head) => {
                                        // Sync to get the new messages
                                        if let Err(e) = self.sync_with_chat_state(chat_manager).await {
                                            error!("Failed to sync after sending message: {:?}", e);
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to send message: {:?}", e);
                                        let error_message = ChatMessage::from_message(
                                            Some(format!("error-{}", Uuid::new_v4())),
                                            None,
                                            Message {
                                                role: Role::System,
                                                content: vec![MessageContent::Text {
                                                    text: format!("Error: {}", e),
                                                }],
                                            },
                                        );
                                        self.add_message_to_chain(error_message);
                                    }
                                }
                                
                                self.waiting_for_response = false;
                            }
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



    /// Handle special commands
    fn handle_command(&mut self, command: &str) {
        match command {
            "/help" => {
                self.toggle_help();
            }
            "/clear" => {
                self.clear_conversation();
                let clear_message = ChatMessage::from_message(
                    Some(format!("system-{}", Uuid::new_v4())),
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text {
                            text: "Conversation cleared".to_string(),
                        }],
                    },
                );
                self.add_message_to_chain(clear_message);
            }
            "/debug" => {
                self.debug = !self.debug;
                let status_msg = if self.debug { "Debug mode enabled" } else { "Debug mode disabled" };
                let system_message = ChatMessage::from_message(
                    None,
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: status_msg.to_string() }],
                    },
                );
                self.add_message(system_message);
            }
            "/status" => {
                let status_text = format!(
                    "Client head: {:?}, Messages: {}, Chain length: {}",
                    self.client_head,
                    self.messages.len(),
                    self.message_chain.len()
                );
                let status_message = ChatMessage::from_message(
                    Some(format!("system-{}", Uuid::new_v4())),
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: status_text }],
                    },
                );
                self.add_message_to_chain(status_message);
            }
            "/sync" => {
                // Add a manual sync command for debugging
                let sync_message = ChatMessage::from_message(
                    Some(format!("system-{}", Uuid::new_v4())),
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text {
                            text: "Manual sync requested (will sync on next opportunity)".to_string(),
                        }],
                    },
                );
                self.add_message_to_chain(sync_message);
            }
            _ => {
                let error_msg = format!("Unknown command: {}. Type /help for available commands.", command);
                let system_message = ChatMessage::from_message(
                    None,
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text { text: error_msg }],
                    },
                );
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

    /// Sync with the chat-state actor by fetching new messages from our head to the server's head
    pub async fn sync_with_chat_state(&mut self, chat_manager: &mut ChatManager) -> Result<()> {
        info!("Syncing with chat-state actor");
        
        // Get the current head from the chat-state actor
        let server_head = chat_manager.get_current_head().await?;
        
        // If server head is None, there's no conversation yet
        let Some(server_head) = server_head else {
            info!("No conversation exists on server yet");
            return Ok(());
        };
        
        // If our client head matches the server head, we're already in sync
        if self.client_head.as_ref() == Some(&server_head) {
            debug!("Already in sync with server head: {}", server_head);
            return Ok(());
        }
        
        info!("Server head: {}, Client head: {:?}", server_head, self.client_head);
        
        // Fetch new messages from server head back to our client head
        let new_messages = chat_manager.get_messages_since_head(&server_head, &self.client_head).await?;
        
        info!("Fetched {} new messages", new_messages.len());
        
        // Add new messages to our state
        for message in new_messages {
            self.add_message_to_chain(message);
        }
        
        // Update our client head
        self.client_head = Some(server_head);
        
        info!("Sync complete. New client head: {}", self.client_head.as_ref().unwrap());
        Ok(())
    }
    
    /// Add a message to the chain, maintaining the linked structure
    fn add_message_to_chain(&mut self, message: ChatMessage) {
        let message_id = message.id.as_ref().unwrap().clone();
        
        // Store the message
        self.messages_by_id.insert(message_id.clone(), message.clone());
        
        // Add to chain if not already present
        if !self.message_chain.contains(&message_id) {
            self.message_chain.push(message_id);
        }
        
        // Rebuild the messages vector for UI compatibility
        self.rebuild_messages_vector();
        
        self.update_scroll();
        self.auto_scroll_to_bottom();
    }
    
    /// Rebuild the messages vector from the chain for UI rendering
    fn rebuild_messages_vector(&mut self) {
        self.messages = self.message_chain
            .iter()
            .filter_map(|id| self.messages_by_id.get(id))
            .cloned()
            .collect();
    }
    
    /// Auto-scroll to bottom after adding messages (keep at bottom by default)
    fn auto_scroll_to_bottom(&mut self) {
        // vertical_scroll = 0 now correctly means show most recent messages (bottom)
        self.vertical_scroll = 0;
    }
    
    /// Clear the conversation (reset chain state)
    pub fn clear_conversation(&mut self) {
        self.messages.clear();
        self.messages_by_id.clear();
        self.message_chain.clear();
        self.client_head = None;
        self.update_scroll();
    }

    /// Add a message to the conversation (legacy method for compatibility)
    pub fn add_message(&mut self, message: ChatMessage) {
        self.messages.push(message);
        self.messages_state = self.messages.len().saturating_sub(1);
        self.update_scroll();
        // Keep at bottom to show new messages
        self.auto_scroll_to_bottom();
    }

    /// Update scroll state based on messages
    pub fn update_scroll(&mut self) {
        // The scroll state will be updated in the UI rendering
        // This method is kept for compatibility but the real scroll state
        // management now happens in render_chat_area()
    }

    /// Scroll messages up (to see older messages)
    pub fn scroll_up(&mut self) {
        // Calculate total lines (this should match the UI calculation)
        let total_lines = self.calculate_total_display_lines();
        let available_height = 20; // This is an estimate, should be passed from UI but this works
        
        if total_lines > available_height {
            let max_scroll = total_lines.saturating_sub(available_height);
            if self.vertical_scroll < max_scroll {
                self.vertical_scroll += 1;
            }
        }
    }

    /// Scroll messages down (to see newer messages)
    pub fn scroll_down(&mut self) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(1);
    }

    /// Calculate total display lines (should match UI calculation)
    fn calculate_total_display_lines(&self) -> usize {
        let mut total_lines = 0;
        let available_width = 70; // Estimate, should be passed from UI but this works for now
        
        for chat_msg in &self.messages {
            let message = chat_msg.as_message();
            // Role header
            total_lines += 1;
            
            // Content lines
            for content in &message.content {
                match content {
                    MessageContent::Text { text } => {
                        let wrapped_text = textwrap::fill(text, available_width);
                        total_lines += wrapped_text.lines().count();
                    }
                    MessageContent::ToolUse { input, .. } => {
                        // Header + ID + Input label + JSON lines
                        total_lines += 3;
                        let input_str = if input.is_null() {
                            "No parameters".to_string()
                        } else {
                            match serde_json::to_string_pretty(input) {
                                Ok(formatted) => formatted,
                                Err(_) => format!("{}", input),
                            }
                        };
                        let wrapped_input = textwrap::fill(&input_str, available_width.saturating_sub(6));
                        total_lines += wrapped_input.lines().count();
                    }
                    MessageContent::ToolResult { content, .. } => {
                        // Header + ID line
                        total_lines += 2;
                        for tool_content in content {
                            match tool_content {
                                mcp_protocol::tool::ToolContent::Text { text } => {
                                    total_lines += 1; // Output label
                                    let wrapped_output = textwrap::fill(text, available_width.saturating_sub(6));
                                    total_lines += wrapped_output.lines().count();
                                }
                                _ => {
                                    total_lines += 1; // Single line for other content types
                                }
                            }
                        }
                    }
                }
            }
            
            // Add line for completion token usage if present
            if chat_msg.is_completion() {
                total_lines += 1;
            }
            
            // Spacing between messages
            total_lines += 1;
        }
        
        total_lines
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

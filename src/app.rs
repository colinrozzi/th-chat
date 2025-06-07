use anyhow::Context;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::stream::StreamExt;
use futures::FutureExt;
use genai_types::MessageContent;
use ratatui::{backend::Backend, Terminal};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use theater::messages::ChannelParticipant;
use theater::TheaterId;
use theater_client::TheaterConnection;
use theater_server::{ManagementCommand, ManagementResponse};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::chat::{ChatManager, ChatMessage, ChatStateResponse};
use crate::config::{CompatibleArgs, LoadingState, LoadingStep, StepStatus};


/// Current input mode
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    Editing,
}

/// Message navigation state
#[derive(Debug, Clone, PartialEq)]
pub enum NavigationMode {
    /// Normal scrolling mode (existing behavior)
    Scroll,
    /// Message navigation mode (vim-style j/k navigation)
    Navigate,
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
    /// Loading state during startup
    pub loading_state: Option<LoadingState>,
    /// Whether we're in loading mode
    pub is_loading: bool,
    /// Boot-style loading steps
    pub loading_steps: Vec<LoadingStep>,
    /// Current step being processed
    pub current_step_index: usize,
    /// Boot animation state (for the blinking cursor effect)
    pub boot_cursor_visible: bool,
    /// Last boot animation update
    pub last_boot_update: Instant,
    /// Current navigation mode
    pub navigation_mode: NavigationMode,
    /// Index of currently selected message (when in Navigate mode)
    pub selected_message_index: Option<usize>,
    /// Whether to show message selection highlighting
    pub show_message_selection: bool,
    /// Set of collapsed message indices
    pub collapsed_messages: std::collections::HashSet<usize>,
    /// Number of lines in the current input text
    pub input_lines: usize,
    /// Which line the cursor is currently on (0-based)
    pub cursor_line: usize,
    /// Column position on the current line (0-based)
    pub cursor_col: usize,
    /// Tool display mode
    pub tool_display_mode: crate::config::ToolDisplayMode,
}

impl Default for App {
    fn default() -> App {
        App {
            input_mode: InputMode::Normal,
            input: String::new(),
            input_cursor_position: 0,
            messages: Vec::new(),

            should_quit: false,
            connection_status: "Disconnected".to_string(),
            waiting_for_response: false,
            thinking_dots: ".".to_string(),
            last_thinking_update: Instant::now(),

            show_help: false,
            scroll_state: ratatui::widgets::ScrollbarState::default(),
            vertical_scroll: 0,
            client_head: None,
            messages_by_id: HashMap::new(),
            message_chain: Vec::new(),
            loading_state: Some(LoadingState::ConnectingToServer("".to_string())),
            is_loading: true,
            loading_steps: Vec::new(),
            current_step_index: 0,
            boot_cursor_visible: true,
            last_boot_update: Instant::now(),
            navigation_mode: NavigationMode::Scroll,
            selected_message_index: None,
            show_message_selection: false,
            collapsed_messages: std::collections::HashSet::new(),
            input_lines: 1,
            cursor_line: 0,
            cursor_col: 0,
            tool_display_mode: crate::config::ToolDisplayMode::default(),
        }
    }
}

impl App {
    pub fn new(_debug: bool) -> Self {
        let loading_steps = vec![
            LoadingStep {
                message: "Initializing th-chat system".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Connecting to Theater runtime server".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Loading chat-state actor manifest".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Starting chat-state actor instance".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Opening communication channel".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Initializing MCP server connections".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Saving session data".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Syncing conversation history".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Preparing chat interface".to_string(),
                status: StepStatus::Pending,
            },
        ];

        App {
            loading_steps,
            ..Default::default()
        }
    }

    /// Main application loop with chain synchronization

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
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::F(1) => {
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
                // Vim-style navigation
                KeyCode::Char('j') => match self.navigation_mode {
                    NavigationMode::Scroll => self.scroll_down(),
                    NavigationMode::Navigate => self.navigate_message_down(),
                },
                KeyCode::Char('k') => match self.navigation_mode {
                    NavigationMode::Scroll => self.scroll_up(),
                    NavigationMode::Navigate => self.navigate_message_up(),
                },
                // Toggle between scroll and navigate modes
                KeyCode::Char('v') => {
                    self.toggle_navigation_mode();
                }
                // Traditional arrow key navigation (still works in scroll mode)
                KeyCode::Up => match self.navigation_mode {
                    NavigationMode::Scroll => self.scroll_up(),
                    NavigationMode::Navigate => self.navigate_message_up(),
                },
                KeyCode::Down => match self.navigation_mode {
                    NavigationMode::Scroll => self.scroll_down(),
                    NavigationMode::Navigate => self.navigate_message_down(),
                },
                KeyCode::Char('c') => {
                    // Toggle collapse/expand for selected message (only in Navigate mode)
                    if self.navigation_mode == NavigationMode::Navigate {
                        self.toggle_message_collapse();
                    }
                }
                KeyCode::Char('t') => {
                    // Cycle tool display mode
                    self.cycle_tool_display_mode();
                }
                KeyCode::Char('T') => {
                    // Auto-collapse tool-heavy messages
                    self.auto_collapse_tool_messages();
                }
                KeyCode::Char('h') => {
                    self.toggle_help();
                }
                KeyCode::F(1) => {
                    self.toggle_help();
                }
                KeyCode::Enter => {
                    // Submit the message
                    if let Some(message) = self.submit_message() {
                        return Ok(Some(message));
                    }
                }
                _ => {}
            },
            InputMode::Editing => match key_event.code {
                // Ctrl+Enter to send message (instead of just Enter)
                // Regular Enter for newline
                KeyCode::Enter => {
                    // Insert a newline in the input
                    self.insert_newline();
                }
                // Ctrl+A to move to start
                KeyCode::Char('a') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input_cursor_position = 0;
                }
                // Ctrl+E to move to end
                KeyCode::Char('e') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input_cursor_position = self.input.chars().count();
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
                // Add up/down arrow support for multi-line navigation
                KeyCode::Up => {
                    // Calculate available width (we'll improve this later)
                    let available_width = 80;
                    self.move_cursor_up(available_width);
                }
                KeyCode::Down => {
                    let available_width = 80;
                    self.move_cursor_down(available_width);
                }
                // Home/End for line navigation
                KeyCode::Home => {
                    self.move_cursor_to_line_start();
                }
                KeyCode::End => {
                    self.move_cursor_to_line_end();
                }
                KeyCode::Esc => {
                    self.input_mode = InputMode::Normal;
                }
                _ => {}
            },
        }
        Ok(None)
    }

    fn process_channel_message(&mut self, payload: ChatStateResponse) -> Result<()> {
        match payload {
            ChatStateResponse::ChatMessage { message } => {
                info!("Received chat message: {:?}", message);
                self.add_message_to_chain(message);
            }
            ChatStateResponse::Head { head } => {
                info!("Received head update: {:?}", head);
                self.client_head = head;
            }
            ChatStateResponse::Error { error } => {
                error!("Error from server: {:?}", error);
            }
            _ => {
                error!("Unknown message type from server");
            }
        }

        Ok(())
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

        // Recalculate line information if we just inserted a newline
        if new_char == '\n' {
            self.input_lines = self.input.lines().count().max(1);
        }
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

            // Get the character we're about to delete
            let chars: Vec<char> = self.input.chars().collect();
            let deleted_char = chars.get(from_left_to_current_index).copied();

            let before_char_to_delete = self.input.chars().take(from_left_to_current_index);
            let after_char_to_delete = self.input.chars().skip(current_index);
            self.input = before_char_to_delete.chain(after_char_to_delete).collect();
            self.move_cursor_left();

            // Recalculate line information if we just deleted a newline
            if deleted_char == Some('\n') {
                self.input_lines = self.input.lines().count().max(1);
            }
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

    /// Insert a newline character at cursor position
    pub fn insert_newline(&mut self) {
        self.enter_char('\n');
    }

    /// Calculate cursor position for multi-line input
    pub fn calculate_cursor_position(&mut self, _available_width: usize) {
        if self.input.is_empty() {
            self.cursor_line = 0;
            self.cursor_col = 0;
            self.input_lines = 1;
            return;
        }

        // Count actual lines (split by \n) and find cursor position within those
        let lines: Vec<&str> = self.input.split('\n').collect();
        self.input_lines = lines.len();
        
        let mut char_count = 0;
        
        for (line_idx, line) in lines.iter().enumerate() {
            let line_char_count = line.chars().count();
            
            if char_count + line_char_count >= self.input_cursor_position {
                // Cursor is on this line
                self.cursor_line = line_idx;
                self.cursor_col = self.input_cursor_position - char_count;
                return;
            }
            
            char_count += line_char_count + 1; // +1 for the \n character
        }
        
        // Fallback - cursor at end
        self.cursor_line = lines.len().saturating_sub(1);
        self.cursor_col = lines.last().map(|l| l.chars().count()).unwrap_or(0);
    }

    /// Move cursor up one line
    pub fn move_cursor_up(&mut self, available_width: usize) {
        self.calculate_cursor_position(available_width);

        if self.cursor_line > 0 {
            // Get the actual lines (split by \n)
            let lines: Vec<&str> = self.input.split('\n').collect();

            // Move to the previous line, trying to maintain column position
            let target_line = self.cursor_line - 1;
            let target_line_len = lines[target_line].chars().count();
            let new_col = self.cursor_col.min(target_line_len);

            // Calculate the character position
            let mut new_position = 0;
            for i in 0..target_line {
                new_position += lines[i].chars().count() + 1; // +1 for newline
            }
            new_position += new_col;

            self.input_cursor_position = new_position.min(self.input.chars().count());
        }
    }

    /// Move cursor down one line
    pub fn move_cursor_down(&mut self, available_width: usize) {
        self.calculate_cursor_position(available_width);

        let lines: Vec<&str> = self.input.split('\n').collect();

        if self.cursor_line < lines.len() - 1 {
            // Move to the next line, trying to maintain column position
            let target_line = self.cursor_line + 1;
            let target_line_len = lines[target_line].chars().count();
            let new_col = self.cursor_col.min(target_line_len);

            // Calculate the character position
            let mut new_position = 0;
            for i in 0..target_line {
                new_position += lines[i].chars().count() + 1; // +1 for newline
            }
            new_position += new_col;

            self.input_cursor_position = new_position.min(self.input.chars().count());
        }
    }

    /// Move cursor to the beginning of the current line
    pub fn move_cursor_to_line_start(&mut self) {
        // Find the last newline before current position
        let chars: Vec<char> = self.input.chars().collect();
        let mut pos = self.input_cursor_position;

        while pos > 0 && chars[pos - 1] != '\n' {
            pos -= 1;
        }

        self.input_cursor_position = pos;
    }

    /// Move cursor to the end of the current line
    pub fn move_cursor_to_line_end(&mut self) {
        // Find the next newline after current position
        let chars: Vec<char> = self.input.chars().collect();
        let mut pos = self.input_cursor_position;

        while pos < chars.len() && chars[pos] != '\n' {
            pos += 1;
        }

        self.input_cursor_position = pos;
    }

    /// Get the minimum height needed for the input area
    pub fn get_input_height(&self) -> u16 {
        // Add 2 for borders, plus at least 1 line, max reasonable height
        (self.input_lines + 2).min(10).max(3) as u16
    }

    /// Sync with the chat-state actor by fetching new messages from our head to the server's head


    /// Add a message to the chain, maintaining the linked structure
    fn add_message_to_chain(&mut self, message: ChatMessage) {
        // Generate an ID if the message doesn't have one
        let message_id = match message.id.as_ref() {
            Some(id) => id.clone(),
            None => {
                // Generate a unique ID for messages without one
                let generated_id = format!("msg-{}", Uuid::new_v4());
                warn!("Message has no ID, generating: {}", generated_id);
                generated_id
            }
        };

        // Create a new message with the guaranteed ID
        let mut message_with_id = message.clone();
        message_with_id.id = Some(message_id.clone());

        // Store the message
        self.messages_by_id
            .insert(message_id.clone(), message_with_id);

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
        self.messages = self
            .message_chain
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

    /// Sync conversation history with the chat-state actor
    pub async fn sync_conversation_history(&mut self, chat_manager: &ChatManager) -> Result<()> {
        info!("Syncing conversation history with chat-state actor");

        // Get the current head from the server
        let server_head = match chat_manager.get_current_head().await {
            Ok(head) => head,
            Err(e) => {
                warn!("Failed to get server head: {}", e);
                return Ok(()); // Don't fail the whole process
            }
        };

        info!(
            "Server head: {:?}, Client head: {:?}",
            server_head, self.client_head
        );

        // If heads match, we're already in sync
        if server_head == self.client_head {
            info!("Already in sync with server");
            return Ok(());
        }

        // If we have no messages or heads don't match, get the full history
        if self.message_chain.is_empty() || server_head != self.client_head {
            info!("Getting full conversation history from server");

            match chat_manager.get_history().await {
                Ok(history) => {
                    info!("Received {} messages from history", history.len());

                    // Clear current state and rebuild from history
                    self.clear_conversation();

                    // Add all messages from history
                    for (index, message) in history.iter().enumerate() {
                        let msg_id = message.id.as_ref().map(|s| s.as_str()).unwrap_or("<no-id>");
                        info!(
                            "Loading message {}/{}: id={}, has_content={}",
                            index + 1,
                            history.len(),
                            msg_id,
                            !message.as_message().content.is_empty()
                        );
                        self.add_message_to_chain(message.clone());
                    }

                    // Update our head to match the server
                    self.client_head = server_head;

                    info!(
                        "Successfully synced {} messages from conversation history",
                        history.len()
                    );
                }
                Err(e) => {
                    warn!("Failed to get conversation history: {}", e);
                    // Don't fail - continue with empty state
                }
            }
        }

        Ok(())
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
                        let wrapped_input =
                            textwrap::fill(&input_str, available_width.saturating_sub(6));
                        total_lines += wrapped_input.lines().count();
                    }
                    MessageContent::ToolResult { content, .. } => {
                        // Header + ID line
                        total_lines += 2;
                        for tool_content in content {
                            match tool_content {
                                mcp_protocol::tool::ToolContent::Text { text } => {
                                    total_lines += 1; // Output label
                                    let wrapped_output =
                                        textwrap::fill(text, available_width.saturating_sub(6));
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
        if self.waiting_for_response
            && self.last_thinking_update.elapsed() > Duration::from_millis(500)
        {
            self.thinking_dots = match self.thinking_dots.as_str() {
                "." => "..".to_string(),
                ".." => "...".to_string(),
                "..." => ".".to_string(),
                _ => ".".to_string(),
            };
            self.last_thinking_update = Instant::now();
        }
    }

    /// Set the current loading state (legacy compatibility)


    /// Mark loading as finished
    pub fn finish_loading(&mut self) {
        // Complete any remaining steps
        for step in &mut self.loading_steps {
            if matches!(step.status, StepStatus::Pending | StepStatus::InProgress) {
                step.status = StepStatus::Success;
            }
        }
        self.is_loading = false;
        self.loading_state = None;
    }

    /// Update the current loading step status


    /// Set current step to in-progress and update its message if needed
    pub fn start_loading_step(&mut self, step_index: usize, custom_message: Option<String>) {
        if step_index < self.loading_steps.len() {
            if let Some(msg) = custom_message {
                self.loading_steps[step_index].message = msg;
            }
            self.loading_steps[step_index].status = StepStatus::InProgress;
            self.current_step_index = step_index;
        }
    }

    /// Complete current step successfully
    pub fn complete_current_step(&mut self) {
        if self.current_step_index < self.loading_steps.len() {
            self.loading_steps[self.current_step_index].status = StepStatus::Success;
            self.current_step_index += 1;
        }
    }

    /// Fail current step with error message
    pub fn fail_current_step(&mut self, error: String) {
        if self.current_step_index < self.loading_steps.len() {
            self.loading_steps[self.current_step_index].status = StepStatus::Failed(error);
        }
    }

    /// Update boot animation (blinking cursor effect)
    pub fn update_boot_animation(&mut self) {
        if self.last_boot_update.elapsed() > Duration::from_millis(500) {
            self.boot_cursor_visible = !self.boot_cursor_visible;
            self.last_boot_update = Instant::now();
        }
    }

    /// Check if all loading steps are complete
    pub fn is_loading_complete(&self) -> bool {
        self.loading_steps
            .iter()
            .all(|step| matches!(step.status, StepStatus::Success))
    }

    /// Toggle help popup
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Toggle between scroll and navigate modes
    pub fn toggle_navigation_mode(&mut self) {
        match self.navigation_mode {
            NavigationMode::Scroll => {
                self.navigation_mode = NavigationMode::Navigate;
                self.show_message_selection = true;
                // Start at the most recent message
                if !self.messages.is_empty() {
                    self.selected_message_index = Some(self.messages.len() - 1);
                }
            }
            NavigationMode::Navigate => {
                self.navigation_mode = NavigationMode::Scroll;
                self.show_message_selection = false;
                self.selected_message_index = None;
            }
        }
    }

    /// Navigate to previous message (vim k)
    pub fn navigate_message_up(&mut self) {
        if self.navigation_mode == NavigationMode::Navigate && !self.messages.is_empty() {
            match self.selected_message_index {
                Some(index) if index > 0 => {
                    self.selected_message_index = Some(index - 1);
                    self.ensure_selected_message_visible();
                }
                None => {
                    // Start from the bottom if no selection
                    self.selected_message_index = Some(self.messages.len() - 1);
                    self.ensure_selected_message_visible();
                }
                _ => {} // Already at top
            }
        }
    }

    /// Navigate to next message (vim j)  
    pub fn navigate_message_down(&mut self) {
        if self.navigation_mode == NavigationMode::Navigate && !self.messages.is_empty() {
            match self.selected_message_index {
                Some(index) if index < self.messages.len() - 1 => {
                    self.selected_message_index = Some(index + 1);
                    self.ensure_selected_message_visible();
                }
                None => {
                    // Start from the top if no selection
                    self.selected_message_index = Some(0);
                    self.ensure_selected_message_visible();
                }
                _ => {} // Already at bottom
            }
        }
    }

    /// Ensure the selected message is visible on screen
    fn ensure_selected_message_visible(&mut self) {
        if let Some(selected_index) = self.selected_message_index {
            // Calculate the line position of the selected message
            let mut line_count = 0;
            let available_width = 70; // Estimate, should match UI calculation

            for (msg_index, chat_msg) in self.messages.iter().enumerate() {
                let message_start_line = line_count;

                // Count lines for this message (similar to UI calculation)
                let message = chat_msg.as_message();
                line_count += 1; // Role header

                for content in &message.content {
                    line_count += match content {
                        MessageContent::Text { text } => (text.len() / available_width) + 1,
                        MessageContent::ToolUse { .. } => 6, // Estimated lines for tool use
                        MessageContent::ToolResult { content, .. } => {
                            3 + content.len() * 2 // Estimated lines for tool result
                        }
                    };
                }

                if let Some(_completion) = chat_msg.as_completion() {
                    line_count += 1; // Token usage line
                }
                line_count += 1; // Empty line between messages

                // If this is our selected message, adjust scroll to make it visible
                if msg_index == selected_index {
                    let available_height = 20; // Estimate
                    let total_lines = self.calculate_total_display_lines();

                    if total_lines > available_height {
                        let max_scroll = total_lines.saturating_sub(available_height);

                        // If message is above current view, scroll up to show it
                        if message_start_line
                            < (total_lines - available_height - self.vertical_scroll)
                        {
                            self.vertical_scroll = max_scroll.saturating_sub(message_start_line);
                        }
                        // If message is below current view, scroll down to show it
                        else if message_start_line >= (total_lines - self.vertical_scroll) {
                            self.vertical_scroll =
                                (total_lines - message_start_line).saturating_sub(available_height);
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Get the currently selected message index for UI highlighting
    pub fn get_selected_message_index(&self) -> Option<usize> {
        if self.show_message_selection {
            self.selected_message_index
        } else {
            None
        }
    }

    /// Toggle collapse/expand state for the currently selected message
    pub fn toggle_message_collapse(&mut self) {
        if let Some(selected_index) = self.selected_message_index {
            if selected_index < self.messages.len() {
                if self.collapsed_messages.contains(&selected_index) {
                    self.collapsed_messages.remove(&selected_index);
                } else {
                    self.collapsed_messages.insert(selected_index);
                }
            }
        }
    }

    /// Check if a message is collapsed
    pub fn is_message_collapsed(&self, index: usize) -> bool {
        self.collapsed_messages.contains(&index)
    }

    /// Initialize loading steps for session-aware loading
    pub fn initialize_loading_steps(&mut self) {
        self.loading_steps = vec![
            LoadingStep {
                message: "Initializing session".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Connecting to Theater server".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Starting chat-state actor".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Opening communication channel".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Retrieving conversation metadata".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Syncing conversation history".to_string(),
                status: StepStatus::Pending,
            },
            LoadingStep {
                message: "Preparing chat interface".to_string(),
                status: StepStatus::Pending,
            },
        ];
        self.current_step_index = 0;
        self.is_loading = true;
    }

    /// Main application loop with session context
    pub async fn run_with_session_context<B: Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
        chat_manager: &mut crate::chat::ChatManager,
        args: &CompatibleArgs,
        session_manager: &crate::session_manager::SessionManager,
        session_data: &mut crate::session_manager::SessionData,
    ) -> Result<()> {
        info!("Starting session-aware chat loop for '{}'", session_data.name);
        
        let server_addr: SocketAddr = args.server.parse().context("Invalid server address")?;
        let mut connection = TheaterConnection::new(server_addr);
        
        // Send the initial messages to the server to listen for head and message updates
        let message = ManagementCommand::OpenChannel {
            actor_id: ChannelParticipant::Actor(
                TheaterId::parse(&chat_manager.actor_id).expect("Invalid actor ID"),
            ),
            initial_message: vec![],
        };

        info!("Sending initial message to server: {:?}", message);
        connection
            .send(message)
            .await
            .context("Failed to send initial message to Theater server")?;

        self.connection_status = format!("Connected to {} (Session: {})", server_addr, session_data.name);

        let mut reader = EventStream::new();
        let mut message_count = session_data.message_count;

        loop {
            // Update animations
            self.update_thinking_animation();
            if self.is_loading {
                self.update_boot_animation();
            }

            terminal.draw(|f| crate::ui::render(f, self, args))?;

            if self.should_quit {
                break;
            }
            
            let input_event = reader.next().fuse();
            tokio::select! {
                msg = connection.receive().fuse() => {
                    info!("Received message from server: {:?}", msg);

                    match msg {
                        Ok(ManagementResponse::ChannelMessage { channel_id, sender_id: _, message }) => {
                            info!("Received message from channel {}: {:?}", channel_id, message);
                            if let Ok(payload) = serde_json::from_slice::<crate::chat::ChatStateResponse>(&message) {
                                // Check if it's a ChatMessage before processing
                                let is_chat_message = matches!(payload, crate::chat::ChatStateResponse::ChatMessage { .. });
                                
                                let _ = self.process_channel_message(payload);
                                
                                // Update session metadata when we get new messages
                                if is_chat_message {
                                    message_count += 1;
                                    session_data.message_count = message_count;
                                    session_data.update_access_time();
                                    
                                    // Periodically save session state
                                    if message_count % 5 == 0 {
                                        if let Err(e) = session_manager.save_session(session_data) {
                                            warn!("Failed to save session state: {}", e);
                                        }
                                    }
                                }
                            } else {
                                error!("Failed to parse message payload");
                            }
                        }
                        Ok(ManagementResponse::ChannelOpened { channel_id, .. }) => {
                            info!("Channel opened: {}", channel_id);
                        }
                        Ok(ManagementResponse::ChannelClosed { .. }) => {
                            info!("Channel closed by server");
                            break;
                        }
                        Err(e) => {
                            error!("Error receiving message: {:?}", e);
                            break;
                        }
                        _ => {
                            error!("Unexpected message type {}", msg.unwrap_err());
                            break;
                        }
                    }
                }

                event = input_event => {
                    match event {
                        Some(Ok(event)) => {
                            if let Event::Key(key_event) = event {
                                if let Some(message) = self.handle_key_event(key_event)? {
                                    chat_manager.send_message(message.clone()).await?;
                                    chat_manager.request_generation().await?;
                                    
                                    // Update session metadata for sent messages
                                    session_data.update_access_time();
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!("Error reading event: {:?}", e);
                        }
                        None => {
                            // Stream closed
                            error!("Event stream closed");
                            break;
                        }
                    }
                }
            }
        }

        // Update final session state
        session_data.message_count = message_count;
        session_data.update_access_time();
        
        // Cleanup
        chat_manager.cleanup().await?;
        Ok(())
    }

    /// Cycle tool display mode
    pub fn cycle_tool_display_mode(&mut self) {
        self.tool_display_mode = self.tool_display_mode.cycle();
        // Could add a status message here later if needed
        info!("Tool display mode changed to: {}", self.tool_display_mode.display_name());
    }

    /// Auto-collapse tool-heavy messages
    pub fn auto_collapse_tool_messages(&mut self) {
        let mut collapsed_count = 0;
        for (index, message) in self.messages.iter().enumerate() {
            let tool_count = message.as_message().content.iter()
                .filter(|content| matches!(content, 
                    genai_types::MessageContent::ToolUse { .. } | 
                    genai_types::MessageContent::ToolResult { .. }
                ))
                .count();
            
            // Auto-collapse messages with 3+ tool calls
            if tool_count >= 3 && !self.collapsed_messages.contains(&index) {
                self.collapsed_messages.insert(index);
                collapsed_count += 1;
            }
        }
        
        if collapsed_count > 0 {
            info!("Auto-collapsed {} tool-heavy messages", collapsed_count);
        }
    }
}
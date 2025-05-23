use anyhow::Context;
use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures::stream::StreamExt;
use futures::FutureExt;
use genai_types::{messages::Role, Message, MessageContent};
use ratatui::{backend::Backend, Terminal};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use theater::client::TheaterConnection;
use theater::messages::ChannelParticipant;
use theater::theater_server::ManagementCommand;
use theater::theater_server::ManagementResponse;
use theater::TheaterId;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::chat::{ChatManager, ChatMessage, ChatStateResponse};
use crate::config::{Args, LoadingState, LoadingStep, StepStatus};
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
            loading_state: Some(LoadingState::ConnectingToServer("".to_string())),
            is_loading: true,
            loading_steps: Vec::new(),
            current_step_index: 0,
            boot_cursor_visible: true,
            last_boot_update: Instant::now(),
        }
    }
}

impl App {
    pub fn new(debug: bool) -> Self {
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
                message: "Preparing chat interface".to_string(),
                status: StepStatus::Pending,
            },
        ];

        App {
            debug,
            loading_steps,
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
        let server_addr: SocketAddr = args.server.parse().context("Invalid server address")?;

        let mut connection = TheaterConnection::new(server_addr);
        info!("Attempting to connect to Theater server...");
        connection
            .connect()
            .await
            .context("Failed to connect to Theater server")?;

        // send the initial messages to the server to listen for head and message updates
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

        self.connection_status = format!("Connected to {}", server_addr);

        let mut reader = EventStream::new();

        loop {
            // Update animations
            self.update_thinking_animation();
            if self.is_loading {
                self.update_boot_animation();
            }

            terminal.draw(|f| ui::render(f, self, args))?;

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
                            if let Ok(payload) = serde_json::from_slice::<ChatStateResponse>(&message) {
                                let _ = self.process_channel_message(payload);
                            } else {
                                error!("Failed to parse message payload");
                            }
                        }
                        Ok(ManagementResponse::ChannelClosed { .. }) => {
                            info!("Channel closed by server");
                            break;
                        }
                        Err(e) => {
                            error!("Error receiving message: {:?}", e);
                        }
                        _ => {
                            error!("Unexpected message type");
                        }
                    }
                }

                event = input_event => {
                    match event {
                        Some(Ok(event)) => {
                            if let Event::Key(key_event) = event {
                                if let Some(message) = self.handle_key_event(key_event)? {
                                    // Handle special commands
                                    if message.starts_with('/') {
                                        self.handle_command(&message[1..]);
                                    } else {
                                        chat_manager.send_message(message.clone()).await?;
                                        chat_manager.request_generation().await?;
                                    }
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
                let status_msg = if self.debug {
                    "Debug mode enabled"
                } else {
                    "Debug mode disabled"
                };
                let system_message = ChatMessage::from_message(
                    None,
                    None,
                    Message {
                        role: Role::System,
                        content: vec![MessageContent::Text {
                            text: status_msg.to_string(),
                        }],
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
                            text: "Manual sync requested (will sync on next opportunity)"
                                .to_string(),
                        }],
                    },
                );
                self.add_message_to_chain(sync_message);
            }
            _ => {
                let error_msg = format!(
                    "Unknown command: {}. Type /help for available commands.",
                    command
                );
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

        info!(
            "Server head: {}, Client head: {:?}",
            server_head, self.client_head
        );

        // Fetch new messages from server head back to our client head
        let new_messages = chat_manager
            .get_messages_since_head(&server_head, &self.client_head)
            .await?;

        info!("Fetched {} new messages", new_messages.len());

        // Add new messages to our state
        for message in new_messages {
            self.add_message_to_chain(message);
        }

        // Update our client head
        self.client_head = Some(server_head);

        info!(
            "Sync complete. New client head: {}",
            self.client_head.as_ref().unwrap()
        );
        Ok(())
    }

    /// Add a message to the chain, maintaining the linked structure
    fn add_message_to_chain(&mut self, message: ChatMessage) {
        let message_id = message.id.as_ref().unwrap().clone();

        // Store the message
        self.messages_by_id
            .insert(message_id.clone(), message.clone());

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
    pub fn set_loading_state(&mut self, state: LoadingState) {
        self.loading_state = Some(state.clone());
        self.is_loading = true;

        // Map old loading states to new step system
        match state {
            LoadingState::ConnectingToServer(addr) => {
                self.complete_step_if_current(0); // Complete "Initializing th-chat system"
                self.start_loading_step(
                    1,
                    Some(format!("Connecting to Theater server at {}", addr)),
                );
            }
            LoadingState::StartingActor(manifest) => {
                self.complete_step_if_current(1); // Complete server connection
                self.start_loading_step(
                    2,
                    Some(format!(
                        "Loading actor manifest: {}",
                        manifest.split('/').last().unwrap_or(&manifest)
                    )),
                );
            }
            LoadingState::OpeningChannel(actor_id) => {
                self.complete_step_if_current(2); // Complete manifest loading
                self.start_loading_step(3, None); // Start actor instance
                self.complete_step_if_current(3); // Complete actor start
                self.start_loading_step(
                    4,
                    Some(format!("Opening channel to actor {}", &actor_id[..8])),
                );
            }
            LoadingState::InitializingMcp(_) => {
                self.complete_step_if_current(4); // Complete channel opening
                self.start_loading_step(5, None);
            }
            LoadingState::Ready => {
                // Complete all remaining steps
                for i in 0..self.loading_steps.len() {
                    if !matches!(self.loading_steps[i].status, StepStatus::Success) {
                        self.loading_steps[i].status = StepStatus::Success;
                    }
                }
                self.current_step_index = self.loading_steps.len();
            }
        }
    }

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
    pub fn set_loading_step_status(&mut self, step_index: usize, status: StepStatus) {
        if step_index < self.loading_steps.len() {
            let should_advance = matches!(status, StepStatus::Success);
            self.loading_steps[step_index].status = status;

            // Update current step index to the next pending step
            if should_advance {
                self.current_step_index = step_index + 1;
            }
        }
    }

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

    /// Helper method for step completion during legacy loading state transitions
    fn complete_step_if_current(&mut self, step_index: usize) {
        if self.current_step_index == step_index && step_index < self.loading_steps.len() {
            self.loading_steps[step_index].status = StepStatus::Success;
            self.current_step_index += 1;
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

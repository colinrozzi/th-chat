use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use console::{Style, Term};
use genai_types::{messages::Role, Message, MessageContent};
use mcp_protocol::tool::ToolContent;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self};
use std::net::SocketAddr;
use std::time::Duration;
use theater::client::TheaterConnection;
use theater::id::TheaterId;
use theater::theater_server::{ManagementCommand, ManagementResponse};

/// Command line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Address of the Theater server
    #[clap(long, env = "THEATER_SERVER_ADDRESS", default_value = "127.0.0.1:9000")]
    server: String,

    /// Model to use
    #[clap(
        long,
        env = "THEATER_CHAT_MODEL",
        default_value = "gemini-2.5-flash-preview-04-17"
    )]
    model: String,

    /// Provider to use
    #[clap(long, env = "THEATER_CHAT_PROVIDER", default_value = "google")]
    provider: String,

    /// Temperature setting (0.0 to 1.0)
    #[clap(long, env = "THEATER_CHAT_TEMPERATURE")]
    temperature: Option<f32>,

    /// Maximum tokens to generate
    #[clap(long, env = "THEATER_CHAT_MAX_TOKENS", default_value = "65535")]
    max_tokens: u32,

    /// System prompt
    #[clap(long, env = "THEATER_CHAT_SYSTEM_PROMPT")]
    system_prompt: Option<String>,

    /// Conversation title
    #[clap(long, env = "THEATER_CHAT_TITLE", default_value = "CLI Chat")]
    title: String,

    /// Debug mode to print all responses
    #[clap(long, default_value = "false")]
    debug: bool,

    /// Path to MCP servers configuration file (JSON)
    #[clap(long, env = "THEATER_CHAT_MCP_CONFIG")]
    mcp_config: Option<String>,
}

// Chat state actor manifest path
const CHAT_STATE_ACTOR_MANIFEST: &str =
    "/Users/colinrozzi/work/actor-registry/chat-state/manifest.toml";

/// Conversation state tracking
#[derive(Debug, Clone)]
struct ConversationState {
    actor_id: String,
    last_known_head: Option<String>,
}

/// Chat message structure matching the chat-state actor
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub message: Message,
}

impl ConversationState {
    fn new(actor_id: String) -> Self {
        Self {
            actor_id,
            last_known_head: None,
        }
    }

    /// Update the tracked head after displaying messages
    fn update_head(&mut self, new_head: String) {
        self.last_known_head = Some(new_head);
    }

    /// Get all new messages since our last known head
    async fn get_new_messages(
        &self,
        connection: &mut TheaterConnection,
        new_head: &str,
        args: &Args,
    ) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        let mut current_id = Some(new_head.to_string());
        
        // Traverse backwards from new head until we reach our last known head (or the beginning)
        while let Some(id) = current_id {
            // Skip if this is our last known head (we don't want to re-display it)
            if Some(&id) == self.last_known_head.as_ref() {
                break;
            }

            // Get the message
            match self.get_message_by_id(connection, &id, args).await? {
                Some(chat_message) => {
                    messages.push(chat_message.message.clone());
                    current_id = chat_message.parent_id;
                }
                None => {
                    // Message not found, stop traversal
                    if args.debug {
                        println!("DEBUG - Message not found: {}", id);
                    }
                    break;
                }
            }
        }

        // Reverse to get chronological order (oldest to newest)
        messages.reverse();
        Ok(messages)
    }

    /// Get a specific message by ID from the chat-state actor
    async fn get_message_by_id(
        &self,
        connection: &mut TheaterConnection,
        message_id: &str,
        args: &Args,
    ) -> Result<Option<ChatMessage>> {
        let actor_id_parsed: TheaterId = self.actor_id.parse().context("Failed to parse actor ID")?;
        
        let message_request = json!({
            "type": "get_message",
            "message_id": message_id
        });

        if args.debug {
            println!("DEBUG - Requesting message ID: {}", message_id);
        }

        // Send the message request
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message request to actor")?;

        // Wait for response
        loop {
            let resp = connection.receive().await?;

            if args.debug {
                println!("DEBUG - Received message response: {:?}", resp);
            }

            match &resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    match serde_json::from_slice::<serde_json::Value>(message) {
                        Ok(response_value) => {
                            // Check if we got a chat_message response
                            if let Some(message_obj) = response_value.get("message") {
                                let chat_message: ChatMessage = serde_json::from_value(message_obj.clone())
                                    .context("Failed to deserialize chat message")?;
                                return Ok(Some(chat_message));
                            }
                            // Check for error response
                            else if let Some(error) = response_value.get("error") {
                                if let Some(code) = error.get("code").and_then(|c| c.as_str()) {
                                    if code == "404" {
                                        return Ok(None); // Message not found
                                    }
                                }
                                return Err(anyhow::anyhow!("Error getting message: {}", error));
                            }
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Failed to parse message response: {}", e));
                        }
                    }
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Theater error: {:?}", error));
                }
                _ => {
                    if args.debug {
                        println!("Unexpected response while getting message: {:?}", resp);
                    }
                }
            }
        }
    }
}

/// Read and parse the MCP servers configuration file
fn read_mcp_config(path: &str) -> Result<Vec<serde_json::Value>> {
    // Read the file
    let config_content = std::fs::read_to_string(path)
        .context(format!("Failed to read MCP config file: {}", path))?;

    // Parse as JSON
    let config: Vec<serde_json::Value> = serde_json::from_str(&config_content)
        .context(format!("Failed to parse MCP config file as JSON: {}", path))?;

    Ok(config)
}

/// Display all new messages in the conversation
async fn display_new_messages(
    conversation_state: &mut ConversationState,
    connection: &mut TheaterConnection,
    new_head: &str,
    args: &Args,
) -> Result<()> {
    let new_messages = conversation_state.get_new_messages(connection, new_head, args).await?;
    
    if new_messages.is_empty() {
        if args.debug {
            println!("{}", "No new messages to display".yellow());
        }
        return Ok(());
    }

    println!(); // Add spacing before messages
    
    for (i, message) in new_messages.iter().enumerate() {
        // Add separator between messages (except for the first one)
        if i > 0 {
            println!("{}", "─".repeat(50).dimmed());
        }

        // Display role header
        let role_display = match message.role.as_str() {
            "user" => "User".cyan().bold(),
            "assistant" => "Assistant".green().bold(),
            _ => message.role.as_str().white().bold(),
        };
        
        println!("{}", role_display);
        
        // Display the message content
        display_rich_message(message);
        
        if args.debug {
            println!("{}", format!("Message role: {}", message.role).dimmed());
        }
    }
    
    // Update our tracked head
    conversation_state.update_head(new_head.to_string());
    
    Ok(())
}

/// Display a Message with rich formatting showing all content types
fn display_rich_message(message: &Message) {
    for content in &message.content {
        match content {
            MessageContent::Text { text } => {
                println!("{}", text);
            }
            
            MessageContent::ToolUse { id, name, input } => {
                println!("{}", "🔧 Tool Call".bright_cyan().bold());
                println!("  {}: {}", "Name".bright_white().bold(), name.bright_yellow());
                println!("  {}: {}", "ID".bright_white().bold(), id.dimmed());
                
                // Pretty print the input parameters
                if let Ok(pretty_input) = serde_json::to_string_pretty(input) {
                    println!("  {}:", "Parameters".bright_white().bold());
                    for line in pretty_input.lines() {
                        let trimmed = line.trim();
                        if trimmed != "{" && trimmed != "}" {
                            println!("    {}", line.cyan());
                        }
                    }
                } else {
                    println!("  {}: {:?}", "Parameters".bright_white().bold(), input);
                }
                println!();
            }
            
            MessageContent::ToolResult { tool_use_id, content: tool_content, is_error } => {
                let status_text = if is_error.unwrap_or(false) { 
                    "❌ Tool Result (ERROR)".bright_red().bold() 
                } else { 
                    "✅ Tool Result".bright_green().bold() 
                };
                
                println!("{}", status_text);
                println!("  {}: {}", "Tool Use ID".bright_white().bold(), tool_use_id.dimmed());
                
                // Display tool result content
                for result_content in tool_content {
                    match result_content {
                        ToolContent::Text { text } => {
                            println!("  {}:", "Output".bright_white().bold());
                            for line in text.lines() {
                                println!("    {}", line);
                            }
                        }
                        ToolContent::Image { .. } => {
                            println!("  {}: {}", "Output".bright_white().bold(), "[Image content]".italic().dimmed());
                        }
                        ToolContent::Resource { .. } => {
                            println!("  {}: {}", "Output".bright_white().bold(), "[Resource content]".italic().dimmed());
                        }
                        ToolContent::Audio { .. } => {
                            println!("  {}: {}", "Output".bright_white().bold(), "[Audio content]".italic().dimmed());
                        }
                    }
                }
                println!();
            }
        }
    }
}

/// Extract just the text content from a Message for simple text return
fn extract_text_content(message: &Message) -> Option<String> {
    let mut full_text = String::new();
    
    for content in &message.content {
        if let MessageContent::Text { text } = content {
            if !full_text.is_empty() {
                full_text.push('\n');
            }
            full_text.push_str(text);
        }
    }
    
    if full_text.is_empty() {
        None
    } else {
        Some(full_text)
    }
}

/// Display a summary of message content types for debugging
fn display_message_summary(message: &Message) {
    let mut text_count = 0;
    let mut tool_use_count = 0;
    let mut tool_result_count = 0;
    
    for content in &message.content {
        match content {
            MessageContent::Text { .. } => text_count += 1,
            MessageContent::ToolUse { .. } => tool_use_count += 1,
            MessageContent::ToolResult { .. } => tool_result_count += 1,
        }
    }
    
    println!("{}", "Message Summary:".bright_blue().bold());
    if text_count > 0 {
        println!("  Text blocks: {}", text_count);
    }
    if tool_use_count > 0 {
        println!("  Tool calls: {}", tool_use_count);
    }
    if tool_result_count > 0 {
        println!("  Tool results: {}", tool_result_count);
    }
    println!();
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Parse command line arguments
    let args = Args::parse();

    // Parse server address
    let server_address: SocketAddr = args
        .server
        .parse()
        .context("Invalid server address format")?;

    // Connect to the Theater server
    println!("{}", "Connecting to Theater server...".cyan());
    let mut connection = connect_to_theater(server_address)
        .await
        .context("Failed to connect to Theater server")?;
    println!("{}", "Connected to Theater server".green());

    // Start the chat-state actor
    println!("{}", "Starting chat-state actor...".cyan());
    match start_chat_state_actor(&mut connection, &args).await {
        Ok(actor_id) => {
            println!("{}", "Chat-state actor started".green());

            // Print welcome message
            println!("\n{}", "🎭 Theater Chat".bright_blue().bold());
            println!(
                "{}",
                format!(
                    "Using {} via {}",
                    args.model.to_string().yellow().bold(),
                    args.provider.yellow().bold()
                )
            );
            println!(
                "{}",
                "Type your messages (Ctrl+C to exit, /help for commands)".cyan()
            );
            println!();

            // Enter REPL loop
            run_chat_loop(&mut connection, &actor_id, &args).await?;
        }
        Err(e) => {
            // Print a user-friendly error message with suggestions for fixing common issues
            println!("  {}", e.to_string().red());
        }
    }

    Ok(())
}

/// Connect to the Theater server
async fn connect_to_theater(address: SocketAddr) -> Result<TheaterConnection> {
    let mut connection = TheaterConnection::new(address);
    connection.connect().await?;
    Ok(connection)
}

/// Start the chat-state actor
async fn start_chat_state_actor(connection: &mut TheaterConnection, args: &Args) -> Result<String> {
    // Get MCP servers configuration
    let mcp_servers = if let Some(ref config_path) = args.mcp_config {
        // Read from config file
        println!("Reading MCP config from: {}", config_path);
        match read_mcp_config(config_path) {
            Ok(config) => {
                println!(
                    "Successfully loaded MCP config with {} server(s)",
                    config.len()
                );
                serde_json::to_value(config).unwrap_or_else(|_| json!([]))
            }
            Err(e) => {
                println!("Warning: Failed to load MCP config: {}", e);
                println!("Falling back to default configuration");
                json!([])
            }
        }
    } else {
        // Use default configuration
        println!("Using default MCP configuration");
        json!([])
    };

    // Prepare the initial state for the chat-state actor
    let initial_state = json!({
        "conversation_id": uuid::Uuid::new_v4().to_string(),
        "store_id": null, // Let the actor create a new store
        "config": {
            "model_config": {
                "model": args.model.clone(),
                "provider": args.provider.clone(),
            },
            "temperature": args.temperature,
            "max_tokens": args.max_tokens,
            "system_prompt": args.system_prompt.clone(),
            "title": args.title.clone(),
            "mcp_servers": mcp_servers
        }
    });

    // Read the chat-state actor manifest
    let manifest = std::fs::read_to_string(CHAT_STATE_ACTOR_MANIFEST)
        .context("Failed to read chat-state actor manifest")?;

    // Convert initial state to bytes
    let initial_state_bytes =
        serde_json::to_vec(&initial_state).context("Failed to serialize initial state")?;

    println!(
        "Starting actor with manifest: {}",
        CHAT_STATE_ACTOR_MANIFEST
    );

    // Start the chat-state actor
    connection
        .send(ManagementCommand::StartActor {
            manifest,
            initial_state: Some(initial_state_bytes),
            parent: true,
            subscribe: false,
        })
        .await
        .context("Failed to send StartActor command")?;

    // Get the actor ID from the response
    let actor_id = loop {
        let response = connection.receive().await?;

        if args.debug {
            println!("DEBUG - Received response: {:?}", response);
        }

        match response {
            ManagementResponse::ActorStarted { id } => {
                break id.to_string();
            }
            ManagementResponse::Error { error } => {
                return Err(anyhow::anyhow!("Theater error: {:?}", error));
            }
            _ => {
                // Just log and continue waiting for the correct response
                if args.debug {
                    println!("Waiting for actor start, received: {:?}", response);
                }
            }
        }
    };

    println!("Actor started with ID: {}", actor_id);

    // Configure the actor with model settings
    let settings = json!({
        "type": "update_settings",
        "settings": {
            "model_config": {
                "model": args.model.clone(),
                "provider": args.provider.clone(),
            },
            "temperature": args.temperature,
            "max_tokens": args.max_tokens,
            "system_prompt": args.system_prompt.clone(),
            "title": args.title.clone(),
            "mcp_servers": mcp_servers
        }
    });

    println!("Configuring actor with settings...");

    // Parse the actor ID to use in commands
    let actor_id_parsed: TheaterId = actor_id.parse().context("Failed to parse actor ID")?;

    // Send settings to the actor
    connection
        .send(ManagementCommand::SendActorMessage {
            id: actor_id_parsed,
            data: serde_json::to_vec(&settings).context("Failed to serialize settings")?,
        })
        .await
        .context("Failed to send settings to actor")?;

    // Wait for the settings update acknowledgment
    loop {
        let response = connection.receive().await?;

        if args.debug {
            println!("DEBUG - Received response: {:?}", response);
        }

        match response {
            ManagementResponse::SentMessage { .. } => {
                break;
            }
            ManagementResponse::Error { error } => {
                return Err(anyhow::anyhow!("Error sending settings: {:?}", error));
            }
            _ => {
                // Just log and continue waiting for the correct response
                if args.debug {
                    println!(
                        "Waiting for settings confirmation, received: {:?}",
                        response
                    );
                }
            }
        }
    }

    Ok(actor_id)
}

/// Run the chat loop with conversation state tracking
async fn run_chat_loop(
    connection: &mut TheaterConnection,
    actor_id: &str,
    args: &Args,
) -> Result<()> {
    let term = Term::stdout();
    let user_style = Style::new().cyan().bold();
    let debug_style = Style::new().yellow().dim();

    // Initialize conversation state tracking
    let mut conversation_state = ConversationState::new(actor_id.to_string());

    loop {
        // Get user input
        term.write_str(&format!("{} ", user_style.apply_to(">")))?;
        term.flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim();

        // Check for special commands
        if input.starts_with('/') {
            match input {
                "/exit" => {
                    println!("Exiting...");
                    break;
                }
                "/clear" => {
                    term.clear_screen()?;
                    continue;
                }
                "/help" => {
                    println!("\nAvailable commands:");
                    println!("  {} - Exit the program", "/exit".cyan());
                    println!("  {} - Clear the screen", "/clear".cyan());
                    println!("  {} - Show this help message", "/help".cyan());
                    println!("  {} - Show conversation debug info", "/debug".cyan());
                    println!();

                    println!("\nCurrent settings:");
                    println!("  Model: {}", args.model.green());
                    println!("  Provider: {}", args.provider.green());
                    if let Some(temp) = args.temperature {
                        println!("  Temperature: {}", temp.to_string().green());
                    } else {
                        println!("  Temperature: {}", "default".green());
                    }
                    println!("  Max Tokens: {}", args.max_tokens.to_string().green());
                    if let Some(ref prompt) = args.system_prompt {
                        println!("  System Prompt: {}", prompt.green());
                    }
                    println!("  Title: {}", args.title.green());

                    // Display MCP config file if used
                    if let Some(ref config_path) = args.mcp_config {
                        println!("  MCP Config File: {}", config_path.green());
                    } else {
                        println!("  MCP Config: {}", "Using default configuration".yellow());
                    }
                    println!();
                    continue;
                }
                "/debug" => {
                    println!("\nConversation State:");
                    println!("  Actor ID: {}", conversation_state.actor_id.green());
                    if let Some(ref head) = conversation_state.last_known_head {
                        println!("  Last Known Head: {}", head.green());
                    } else {
                        println!("  Last Known Head: {}", "None".yellow());
                    }
                    println!();
                    continue;
                }
                _ => {
                    println!(
                        "{}",
                        "Unknown command. Type /help for available commands.".yellow()
                    );
                    continue;
                }
            }
        }

        // Skip empty messages
        if input.is_empty() {
            continue;
        }

        // Parse actor ID
        let actor_id_parsed: TheaterId = actor_id.parse().context("Failed to parse actor ID")?;

        // Create a proper Message using the genai_types structs
        let message = Message {
            role: Role::User,
            content: vec![MessageContent::Text {
                text: input.to_string(),
            }],
        };

        // Create and send message to actor
        let add_message_request = json!({
            "type": "add_message",
            "message": message
        });

        if args.debug {
            println!("{}", debug_style.apply_to(format!(
                "DEBUG - Sending message request: {}", add_message_request
            )));
        }

        // Send the message to the actor
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&add_message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message to actor")?;

        // Wait for the request response
        loop {
            let resp = connection.receive().await?;

            if args.debug {
                println!("{}", debug_style.apply_to(format!(
                    "DEBUG - Received response: {:?}", resp
                )));
            }

            match resp {
                ManagementResponse::RequestedMessage { .. } => {
                    break;
                }
                ManagementResponse::Error { error } => {
                    println!("Error from actor: {:?}", error);
                    continue;
                }
                _ => {
                    if args.debug {
                        println!(
                            "Unexpected response while waiting for message response: {:?}",
                            resp
                        );
                    }
                }
            }
        }

        // Create a progress bar for the "thinking" indicator
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner} Model is thinking...")
                .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));

        // Generate completion (tell actor to get response)
        let generate_completion_request = json!({
            "type": "generate_completion"
        });

        if args.debug {
            println!("{}", debug_style.apply_to(format!(
                "DEBUG - Sending completion request: {}", generate_completion_request
            )));
        }

        // Send the completion request to the actor
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&generate_completion_request)
                    .context("Failed to serialize completion request")?,
            })
            .await
            .context("Failed to send completion request to actor")?;

        // Wait for the completion response
        let mut new_head = None;
        let mut error_occurred = false;

        loop {
            let resp = connection.receive().await?;

            if args.debug {
                println!("{}", debug_style.apply_to(format!(
                    "DEBUG - Received completion response: {:?}", resp
                )));
            }

            match &resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    match serde_json::from_slice::<serde_json::Value>(message) {
                        Ok(response_value) => {
                            if let Some(head) = response_value.get("head").and_then(|h| h.as_str()) {
                                new_head = Some(head.to_string());
                                break;
                            } else if let Some(error) = response_value.get("error") {
                                pb.finish_and_clear();
                                println!("Error from actor: {}", error);
                                error_occurred = true;
                                break;
                            } else {
                                if args.debug {
                                    println!("Full completion response: {}", response_value);
                                }
                            }
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            println!("Error parsing completion response: {}", e);
                            println!("Raw response: {}", String::from_utf8_lossy(message));
                            error_occurred = true;
                            break;
                        }
                    }
                }
                ManagementResponse::Error { error } => {
                    pb.finish_and_clear();
                    println!("Error from actor: {:?}", error);
                    error_occurred = true;
                    break;
                }
                _ => {
                    if args.debug {
                        println!("Unexpected response: {:?}", resp);
                    }
                }
            }
        }

        pb.finish_and_clear();

        if error_occurred {
            continue;
        }

        // Display all new messages since our last known head
        if let Some(head_id) = new_head {
            match display_new_messages(&mut conversation_state, connection, &head_id, args).await {
                Ok(()) => {
                    // Success - messages displayed and head updated
                    if args.debug {
                        println!("Successfully displayed new messages, head updated to: {}", head_id);
                    }
                }
                Err(e) => {
                    println!("{}", format!("Error displaying messages: {}", e).red());
                    // Still update head to avoid getting stuck
                    conversation_state.update_head(head_id);
                }
            }
        } else {
            println!("{}", "Failed to get head message ID".red());
        }

        println!(); // Add spacing after response
    }

    Ok(())
}

use anyhow::{Context, Result};
use genai_types::{messages::Role, CompletionResponse, Message, MessageContent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use theater::client::TheaterConnection;
use theater::id::TheaterId;
use theater::theater_server::{ManagementCommand, ManagementResponse};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::{Args, CHAT_STATE_ACTOR_MANIFEST};

/// Chat message structure matching the chat-state actor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Option<String>,
    pub parent_id: Option<String>,
    pub entry: ChatEntry,
}

/// Chat entry that can be either a message or completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatEntry {
    Message(Message),
    Completion(CompletionResponse),
}

/// Helper methods for ChatMessage
impl ChatMessage {
    /// Get the message content as a Message (for backward compatibility)
    pub fn as_message(&self) -> Message {
        match &self.entry {
            ChatEntry::Message(msg) => msg.clone(),
            ChatEntry::Completion(completion) => completion.clone().into(),
        }
    }

    /// Check if this is a completion response
    pub fn is_completion(&self) -> bool {
        matches!(self.entry, ChatEntry::Completion(_))
    }

    /// Get completion response if this is a completion
    pub fn as_completion(&self) -> Option<&CompletionResponse> {
        match &self.entry {
            ChatEntry::Completion(completion) => Some(completion),
            _ => None,
        }
    }

    /// Get the role of the message
    pub fn role(&self) -> Role {
        match &self.entry {
            ChatEntry::Message(msg) => msg.role.clone(),
            ChatEntry::Completion(completion) => completion.role.clone(),
        }
    }

    /// Create a new ChatMessage from a Message (convenience constructor)
    pub fn from_message(id: Option<String>, parent_id: Option<String>, message: Message) -> Self {
        Self {
            id,
            parent_id,
            entry: ChatEntry::Message(message),
        }
    }

    /// Create a new ChatMessage from a CompletionResponse (convenience constructor)
    pub fn from_completion(
        id: Option<String>,
        parent_id: Option<String>,
        completion: CompletionResponse,
    ) -> Self {
        Self {
            id,
            parent_id,
            entry: ChatEntry::Completion(completion),
        }
    }
}

/// Response types from the chat-state actor
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatStateResponse {
    #[serde(rename = "head")]
    Head { head: Option<String> },
    #[serde(rename = "history")]
    History { messages: Vec<ChatMessage> },
    #[serde(rename = "chat_message")]
    ChatMessage { message: ChatMessage },
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
    #[serde(rename = "success")]
    Success,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorInfo {
    code: String,
    message: String,
    details: Option<HashMap<String, String>>,
}

/// Manages the chat connection and state
pub struct ChatManager {
    connection: Arc<Mutex<TheaterConnection>>,
    pub actor_id: String,
    debug: bool,
}

impl ChatManager {
    /// Create a new chat manager and initialize the connection
    pub async fn new(args: &Args) -> Result<Self> {
        info!("Creating new ChatManager");
        debug!("Args: {:?}", args);

        // Connect to Theater server
        info!("Parsing server address: {}", args.server);
        let server_addr: SocketAddr = args.server.parse().context("Invalid server address")?;
        debug!("Parsed server address: {:?}", server_addr);

        info!("Creating Theater connection to {}", server_addr);
        let mut connection = TheaterConnection::new(server_addr);

        info!("Attempting to connect to Theater server...");
        connection
            .connect()
            .await
            .context("Failed to connect to Theater server")?;
        info!("Successfully connected to Theater server");

        // Load MCP configuration
        info!("Loading MCP configuration...");
        let mcp_config = if let Some(ref config_path) = args.mcp_config {
            info!("Using custom MCP config path: {}", config_path);
            match read_mcp_config(config_path) {
                Ok(config) => {
                    info!("Successfully loaded custom MCP config");
                    debug!("MCP config: {:?}", config);
                    Some(config)
                }
                Err(e) => {
                    warn!(
                        "Failed to load custom MCP config: {:?}, continuing without MCP",
                        e
                    );
                    None
                }
            }
        } else {
            info!("Using default MCP config: mcp-config.json");
            match read_mcp_config("mcp-config.json") {
                Ok(config) => {
                    info!("Successfully loaded default MCP config");
                    debug!("MCP config: {:?}", config);
                    Some(config)
                }
                Err(e) => {
                    warn!(
                        "Failed to load default MCP config: {:?}, continuing without MCP",
                        e
                    );
                    None
                }
            }
        };

        // Start chat-state actor
        info!(
            "Starting chat-state actor with manifest: {}",
            CHAT_STATE_ACTOR_MANIFEST
        );
        let start_actor_cmd = ManagementCommand::StartActor {
            manifest: CHAT_STATE_ACTOR_MANIFEST.to_string(),
            initial_state: None,
            parent: false,
            subscribe: false,
        };
        debug!("StartActor command: {:?}", start_actor_cmd);

        info!("Sending StartActor command...");
        connection
            .send(start_actor_cmd)
            .await
            .context("Failed to send StartActor command")?;
        info!("StartActor command sent successfully");

        // Get the actor ID from the response
        info!("Waiting for actor start response...");
        let actor_id = loop {
            debug!("Waiting for response from Theater server...");
            let response = connection.receive().await?;
            debug!("Received response: {:?}", response);

            match response {
                ManagementResponse::ActorStarted { id } => {
                    info!("Actor started successfully with ID: {}", id);
                    break id.to_string();
                }
                ManagementResponse::Error { error } => {
                    error!("Failed to start actor: {:?}", error);
                    return Err(anyhow::anyhow!("Failed to start actor: {:?}", error));
                }
                _ => {
                    debug!("Received unexpected response, continuing to wait...");
                }
            }
        };

        // Configure the actor with settings
        info!("Configuring actor with settings...");
        let settings = json!({
            "type": "update_settings",
            "settings": {
                "model_config": {
                    "model": args.model,
                    "provider": args.provider
                },
                "temperature": args.temperature,
                "max_tokens": args.max_tokens,
                "system_prompt": args.system_prompt,
                "title": args.title,
                "mcp_servers": mcp_config
            }
        });
        debug!("Settings payload: {:?}", settings);

        info!("Parsing actor ID: {}", actor_id);
        let actor_id_parsed: TheaterId = actor_id.parse().context("Failed to parse actor ID")?;
        debug!("Parsed actor ID: {:?}", actor_id_parsed);

        info!("Sending settings to actor...");
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&settings).context("Failed to serialize settings")?,
            })
            .await
            .context("Failed to send settings to actor")?;
        info!("Settings sent successfully");

        // Wait for settings confirmation
        info!("Waiting for settings confirmation...");
        loop {
            debug!("Waiting for settings confirmation response...");
            let response = connection.receive().await?;
            debug!("Received settings response: {:?}", response);

            match response {
                ManagementResponse::RequestedMessage { .. } => {
                    info!("Settings configured successfully");
                    break;
                }
                ManagementResponse::Error { error } => {
                    error!("Failed to configure actor: {:?}", error);
                    return Err(anyhow::anyhow!("Failed to configure actor: {:?}", error));
                }
                _ => {
                    debug!("Received unexpected response, continuing to wait for settings confirmation...");
                }
            }
        }

        let connection = Arc::new(Mutex::new(connection));

        info!(
            "ChatManager created successfully with actor ID: {}",
            actor_id
        );
        Ok(ChatManager {
            connection,
            actor_id,
            debug: args.debug,
        })
    }

    /// Get a specific message by ID from the chat-state actor
    async fn get_message_by_id(&self, message_id: &str) -> Result<Option<ChatMessage>> {
        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let message_request = json!({
            "type": "get_message",
            "message_id": message_id
        });

        // Send the message request
        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message request to actor")?;
        }

        // Wait for response
        loop {
            let resp = {
                let mut conn = self.connection.lock().await;
                conn.receive().await?
            };

            match &resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    match serde_json::from_slice::<serde_json::Value>(message) {
                        Ok(response_value) => {
                            // Check if we got a chat_message response
                            if let Some(message_obj) = response_value.get("message") {
                                let chat_message: ChatMessage =
                                    serde_json::from_value(message_obj.clone())
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
                    // Continue waiting for the correct response
                }
            }
        }
    }

    /// Get the current head from the chat-state actor
    pub async fn get_current_head(&mut self) -> Result<Option<String>> {
        info!("Getting current head from chat-state actor");

        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let get_head_request = json!({
            "type": "get_head"
        });

        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&get_head_request)
                    .context("Failed to serialize get_head request")?,
            })
            .await
            .context("Failed to send get_head request")?;
        }

        // Wait for response
        loop {
            let resp = {
                let mut conn = self.connection.lock().await;
                conn.receive().await?
            };

            match resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    let response: ChatStateResponse = serde_json::from_slice(&message)
                        .context("Failed to parse head response")?;

                    match response {
                        ChatStateResponse::Head { head } => {
                            debug!("Received head: {:?}", head);
                            return Ok(head);
                        }
                        ChatStateResponse::Error { error } => {
                            warn!("Error getting head: {:?}", error);
                            return Ok(None);
                        }
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Unexpected response for get_head: {:?}",
                                response
                            ));
                        }
                    }
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error getting head: {:?}", error));
                }
                _ => {
                    debug!("Ignoring unexpected response while waiting for head");
                }
            }
        }
    }

    /// Get messages from server_head back to client_head (following the chain backward)
    pub async fn get_messages_since_head(
        &self,
        server_head: &str,
        client_head: &Option<String>,
    ) -> Result<Vec<ChatMessage>> {
        info!(
            "Getting messages from {} back to {:?}",
            server_head, client_head
        );

        let mut messages = Vec::new();
        let mut current_id = Some(server_head.to_string());

        // Traverse backwards from server head until we reach our client head (or the beginning)
        while let Some(id) = current_id {
            // Stop if we've reached our client's known head
            if client_head.as_ref() == Some(&id) {
                debug!("Reached client head: {}", id);
                break;
            }

            // Get the message
            match self.get_message_by_id(&id).await? {
                Some(chat_message) => {
                    debug!("Retrieved message: {}", id);
                    messages.push(chat_message.clone());
                    current_id = chat_message.parent_id;
                }
                None => {
                    warn!("Message not found: {}", id);
                    break;
                }
            }
        }

        // Reverse to get chronological order (oldest to newest)
        messages.reverse();
        info!(
            "Retrieved {} messages in chronological order",
            messages.len()
        );
        Ok(messages)
    }

    pub async fn send_message(&mut self, message: String) -> Result<()> {
        info!("Sending message: {}", message);

        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let message_obj = Message {
            role: Role::User,
            content: vec![MessageContent::Text { text: message }],
        };

        let add_message_request = json!({
            "type": "add_message",
            "message": message_obj
        });

        // Send the message
        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&add_message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message to actor")?;
        }

        Ok(())
    }

    pub async fn request_generation(&mut self) -> Result<()> {
        info!("Requesting message generation");

        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let generate_request = json!({
            "type": "generate_completion"
        });

        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&generate_request)
                    .context("Failed to serialize generate request")?,
            })
            .await
            .context("Failed to send generate request")?;
        }

        Ok(())
    }

    /// Send a message and return the new head (don't fetch messages here)
    pub async fn send_message_get_head(&mut self, message: String) -> Result<String> {
        info!("Sending message and getting new head");

        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let message_obj = Message {
            role: Role::User,
            content: vec![MessageContent::Text { text: message }],
        };

        let add_message_request = json!({
            "type": "add_message",
            "message": message_obj
        });

        // Send the message
        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&add_message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message to actor")?;
        }

        // Wait for acknowledgment
        loop {
            let resp = {
                let mut conn = self.connection.lock().await;
                conn.receive().await?
            };

            match resp {
                ManagementResponse::RequestedMessage { .. } => break,
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error adding message: {:?}", error));
                }
                _ => continue,
            }
        }

        // Generate completion
        let generate_request = json!({
            "type": "generate_completion"
        });

        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&generate_request)
                    .context("Failed to serialize generate request")?,
            })
            .await
            .context("Failed to send generate request")?;
        }

        // Wait for completion and get new head
        loop {
            let resp = {
                let mut conn = self.connection.lock().await;
                conn.receive().await?
            };

            match resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    let response: ChatStateResponse = serde_json::from_slice(&message)
                        .context("Failed to parse completion response")?;

                    match response {
                        ChatStateResponse::Head {
                            head: Some(new_head),
                        } => {
                            info!("Received new head after completion: {}", new_head);
                            return Ok(new_head);
                        }
                        ChatStateResponse::Error { error } => {
                            return Err(anyhow::anyhow!(
                                "Error generating completion: {:?}",
                                error
                            ));
                        }
                        _ => continue,
                    }
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error generating completion: {:?}", error));
                }
                _ => continue,
            }
        }
    }

    /// Cleanup resources
    pub async fn cleanup(&self) -> Result<()> {
        let actor_id_parsed: TheaterId = self
            .actor_id
            .parse()
            .context("Failed to parse actor ID for cleanup")?;

        let mut connection = self.connection.lock().await;

        // Send stop actor command
        if let Err(e) = connection
            .send(ManagementCommand::StopActor {
                id: actor_id_parsed,
            })
            .await
        {
            eprintln!("Warning: Failed to send stop actor command: {}", e);
            return Ok(()); // Don't fail cleanup on communication error
        }

        // Wait for confirmation with timeout
        let cleanup_timeout = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.wait_for_stop_confirmation(&mut connection),
        );

        match cleanup_timeout.await {
            Ok(Ok(())) => {
                if self.debug {
                    println!("Actor stopped successfully");
                }
            }
            Ok(Err(e)) => {
                eprintln!("Warning: Error during actor cleanup: {}", e);
            }
            Err(_) => {
                eprintln!("Warning: Actor cleanup timed out");
            }
        }

        Ok(())
    }

    async fn wait_for_stop_confirmation(&self, connection: &mut TheaterConnection) -> Result<()> {
        loop {
            let response = connection.receive().await?;
            match response {
                ManagementResponse::ActorStopped { .. } => {
                    break;
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error stopping actor: {:?}", error));
                }
                _ => {
                    // Continue waiting for the stop confirmation
                }
            }
        }
        Ok(())
    }
}

/// Read and parse the MCP servers configuration file
fn read_mcp_config(path: &str) -> Result<Vec<serde_json::Value>> {
    let config_content = std::fs::read_to_string(path)
        .context(format!("Failed to read MCP config file: {}", path))?;

    let config: Vec<serde_json::Value> = serde_json::from_str(&config_content)
        .context("Failed to parse MCP config as JSON array")?;

    Ok(config)
}

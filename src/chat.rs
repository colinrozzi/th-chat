use anyhow::{Context, Result};
use genai_types::{messages::Role, CompletionResponse, Message, MessageContent};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use theater_client::TheaterConnection;
use theater_server::{ManagementCommand, ManagementResponse};
use theater::id::TheaterId;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::{CompatibleArgs, CHAT_STATE_ACTOR_MANIFEST};

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
    #[serde(rename = "metadata")]
    Metadata {
        conversation_id: String,
        store_id: String,
    },
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
    /// Connect to Theater server
    pub async fn connect_to_server(args: &CompatibleArgs) -> Result<TheaterConnection> {
        info!("Connecting to Theater server");
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

        Ok(connection)
    }

    /// Start the chat-state actor
    pub async fn start_actor(
        connection: &mut TheaterConnection,
        _args: &CompatibleArgs,
    ) -> Result<TheaterId> {
        Self::start_actor_with_session(connection, _args, None).await
    }

    /// Start the chat-state actor with optional session data
    pub async fn start_actor_with_session(
        connection: &mut TheaterConnection,
        _args: &CompatibleArgs,
        session_data: Option<&crate::persistence::SessionData>,
    ) -> Result<TheaterId> {
        info!("Starting chat-state actor");
        
        // Prepare initial state from session data if available
        let initial_state = if let Some(session) = session_data {
            info!(
                "Starting chat-state actor with existing session - conversation_id: {}, store_id: {}",
                session.conversation_id, session.store_id
            );
            let init_data = serde_json::json!({
                "store_id": session.store_id,
                "conversation_id": session.conversation_id,
                "config": null
            });
            Some(serde_json::to_vec(&init_data).context("Failed to serialize session data")?)
        } else {
            info!("Starting chat-state actor with new session");
            // Create minimal init data for new session
            let init_data = serde_json::json!({});
            Some(serde_json::to_vec(&init_data).context("Failed to serialize empty init data")?)
        };

        // Start chat-state actor
        info!(
            "Starting chat-state actor with manifest: {}",
            CHAT_STATE_ACTOR_MANIFEST
        );
        let start_actor_cmd = ManagementCommand::StartActor {
            manifest: CHAT_STATE_ACTOR_MANIFEST.to_string(),
            initial_state,
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
                    break id;
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

        Ok(actor_id)
    }

    /// Open channel and finalize ChatManager creation
    pub async fn open_channel(
        connection: TheaterConnection,
        actor_id: TheaterId,
        args: &CompatibleArgs,
    ) -> Result<Self> {
        Self::open_channel_with_config(connection, actor_id, args, None).await
    }
    
    /// Open channel with explicit configuration (for new config system)
    pub async fn open_channel_with_config(
        mut connection: TheaterConnection,
        actor_id: TheaterId,
        args: &CompatibleArgs,
        config: Option<&crate::config_manager::ConversationConfig>,
    ) -> Result<Self> {
        info!("Opening channel and configuring actor");
        
        // Load MCP configuration - handle both old and new systems
        info!("Loading MCP configuration...");
        let mcp_servers = if let Some(conversation_config) = config {
            // New system: MCP servers are part of the conversation config
            info!("Using MCP servers from conversation configuration ({} servers)", conversation_config.mcp_servers.len());
            debug!("MCP servers: {:?}", conversation_config.mcp_servers);
            conversation_config.mcp_servers.clone()
        } else if let Some(ref config_path) = args.mcp_config {
            // Old system: Load from JSON file and convert to new format
            info!("Using custom MCP config path: {}", config_path);
            match read_mcp_config(config_path) {
                Ok(legacy_config) => {
                    info!("Successfully loaded custom MCP config");
                    debug!("Legacy MCP config: {:?}", legacy_config);
                    // Convert legacy format to new format if needed
                    // For now, assume it's already in the right format
                    serde_json::from_value(serde_json::Value::Array(legacy_config))
                        .unwrap_or_else(|_| vec![])
                }
                Err(e) => {
                    warn!(
                        "Failed to load custom MCP config: {:?}, continuing without MCP",
                        e
                    );
                    vec![]
                }
            }
        } else {
            // Old system: Try default file
            info!("Using default MCP config: mcp-config.json");
            match read_mcp_config("mcp-config.json") {
                Ok(legacy_config) => {
                    info!("Successfully loaded default MCP config");
                    debug!("Legacy MCP config: {:?}", legacy_config);
                    // Convert legacy format to new format if needed
                    serde_json::from_value(serde_json::Value::Array(legacy_config))
                        .unwrap_or_else(|_| vec![])
                }
                Err(e) => {
                    warn!(
                        "Failed to load default MCP config: {:?}, continuing without MCP",
                        e
                    );
                    vec![]
                }
            }
        };

        // Configure the actor with settings
        info!("Configuring actor with settings...");
        let settings = if let Some(conversation_config) = config {
            // New system: Use the full conversation config - exact same structure as chat-state expects
            json!({
                "type": "update_settings",
                "settings": {
                    "model_config": conversation_config.model_config,
                    "temperature": conversation_config.temperature,
                    "max_tokens": conversation_config.max_tokens,
                    "system_prompt": conversation_config.system_prompt,
                    "title": conversation_config.title,
                    "mcp_servers": mcp_servers
                }
            })
        } else {
            // Old system: Use individual args fields
            json!({
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
                    "mcp_servers": mcp_servers
                }
            })
        };
        debug!("Settings payload: {:?}", settings);

        info!("Sending settings to actor...");
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id.clone(),
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
            actor_id: actor_id.to_string(),
            debug: args.debug,
        })
    }

    /// Create a new chat manager and initialize the connection (deprecated - use stepped approach)
    pub async fn new(args: &CompatibleArgs) -> Result<Self> {
        // Use the stepped approach internally
        let mut connection = Self::connect_to_server(args).await?;
        let actor_id = Self::start_actor(&mut connection, args).await?;
        Self::open_channel(connection, actor_id, args).await
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
    pub async fn get_current_head(&self) -> Result<Option<String>> {
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

    /// Get the full conversation history from the chat-state actor
    pub async fn get_history(&self) -> Result<Vec<ChatMessage>> {
        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let history_request = json!({
            "type": "get_history"
        });

        // Send the history request
        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&history_request)
                    .context("Failed to serialize history request")?,
            })
            .await
            .context("Failed to send history request")?;
        }

        // Wait for the response
        loop {
            let mut conn = self.connection.lock().await;
            let response = conn.receive().await?;
            match response {
                ManagementResponse::RequestedMessage { message, .. } => {
                    let response: ChatStateResponse = serde_json::from_slice(&message)
                        .context("Failed to parse history response")?;

                    match response {
                        ChatStateResponse::History { messages } => {
                            info!(
                                "Received conversation history with {} messages",
                                messages.len()
                            );
                            return Ok(messages);
                        }
                        ChatStateResponse::Error { error } => {
                            return Err(anyhow::anyhow!(
                                "Error getting history: {:?}",
                                error
                            ));
                        }
                        _ => continue,
                    }
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error getting history: {:?}", error));
                }
                _ => continue,
            }
        }
    }

    /// Get metadata (conversation_id and store_id) from the chat-state actor
    pub async fn get_metadata(&self) -> Result<(String, String)> {
        let actor_id_parsed: TheaterId =
            self.actor_id.parse().context("Failed to parse actor ID")?;

        let metadata_request = json!({
            "type": "get_metadata"
        });

        // Send the metadata request
        {
            let mut conn = self.connection.lock().await;
            conn.send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed,
                data: serde_json::to_vec(&metadata_request)
                    .context("Failed to serialize metadata request")?,
            })
            .await
            .context("Failed to send metadata request")?;
        }

        // Wait for the response
        loop {
            let mut conn = self.connection.lock().await;
            let response = conn.receive().await?;
            match response {
                ManagementResponse::RequestedMessage { message, .. } => {
                    let response: ChatStateResponse = serde_json::from_slice(&message)
                        .context("Failed to parse metadata response")?;

                    match response {
                        ChatStateResponse::Metadata {
                            conversation_id,
                            store_id,
                        } => {
                            info!(
                                "Received metadata - conversation_id: {}, store_id: {}",
                                conversation_id, store_id
                            );
                            return Ok((conversation_id, store_id));
                        }
                        ChatStateResponse::Error { error } => {
                            return Err(anyhow::anyhow!(
                                "Error getting metadata: {:?}",
                                error
                            ));
                        }
                        _ => continue,
                    }
                }
                ManagementResponse::Error { error } => {
                    return Err(anyhow::anyhow!("Error getting metadata: {:?}", error));
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

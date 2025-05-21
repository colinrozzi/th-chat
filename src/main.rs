use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use console::{Style, Term};
use indicatif::{ProgressBar, ProgressStyle};
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
        default_value = "claude-3-5-sonnet-20240307"
    )]
    model: String,

    /// Provider to use
    #[clap(long, env = "THEATER_CHAT_PROVIDER", default_value = "anthropic")]
    provider: String,

    /// System prompt
    #[clap(long, env = "THEATER_CHAT_SYSTEM_PROMPT")]
    system_prompt: Option<String>,
}

// Chat state actor manifest path
const CHAT_STATE_ACTOR_MANIFEST: &str =
    "/Users/colinrozzi/work/actor-registry/chat-state/manifest.toml";

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
    let actor_id = start_chat_state_actor(&mut connection, &args)
        .await
        .context("Failed to start chat-state actor")?;
    println!("{}", "Chat-state actor started".green());

    // Print welcome message
    println!("\n{}", "🎭 Theater Chat".bright_blue().bold());
    println!(
        "{}",
        "Type your messages (Ctrl+C to exit, /help for commands)".cyan()
    );
    println!();

    // Enter REPL loop
    run_chat_loop(&mut connection, &actor_id).await?;

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
    // Prepare the initial state for the chat-state actor
    let initial_state = json!({
        "conversation_id": uuid::Uuid::new_v4().to_string(),
        "store_id": null, // Let the actor create a new store
    });

    // Read the chat-state actor manifest
    let manifest = std::fs::read_to_string(CHAT_STATE_ACTOR_MANIFEST)
        .context("Failed to read chat-state actor manifest")?;

    // Convert initial state to bytes
    let initial_state_bytes =
        serde_json::to_vec(&initial_state).context("Failed to serialize initial state")?;

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
        if let ManagementResponse::ActorStarted { id } = response {
            break id.to_string();
        }
    };

    // Configure the actor with model settings
    let settings = json!({
        "type": "update_settings",
        "settings": {
            "model_config": {
                "model": args.model.clone(),
                "provider": args.provider.clone(),
            },
            "temperature": null,
            "max_tokens": 65535,
            "system_prompt": args.system_prompt.clone(),
            "title": "CLI Chat",
            "mcp_servers": []
        }
    });

    // Send settings to the actor
    connection
        .send(ManagementCommand::SendActorMessage {
            id: actor_id.parse().context("Failed to parse actor ID")?,
            data: serde_json::to_vec(&settings).context("Failed to serialize settings")?,
        })
        .await
        .context("Failed to send settings to actor")?;

    // Wait for the settings update acknowledgment
    loop {
        let response = connection.receive().await?;
        if let ManagementResponse::SentMessage { .. } = response {
            break;
        }
    }

    Ok(actor_id)
}

/// Run the chat loop
async fn run_chat_loop(connection: &mut TheaterConnection, actor_id: &str) -> Result<()> {
    let term = Term::stdout();
    let user_style = Style::new().cyan().bold();
    let assistant_style = Style::new().green();

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

        // Create and send message to actor
        let add_message_request = json!({
            "type": "add_message",
            "message": {
                "role": "user",
                "content": {
                    "format": null,
                    "data": input
                }
            }
        });

        // Send the message to the actor
        connection
            .send(ManagementCommand::SendActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&add_message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message to actor")?;

        // Wait for the send confirmation
        loop {
            let resp = connection.receive().await?;
            if let ManagementResponse::SentMessage { .. } = resp {
                break;
            }
        }

        // Create a progress bar for the "thinking" indicator
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner} Claude is thinking...")
                .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));

        // Generate completion (tell actor to get response)
        let generate_completion_request = json!({
            "type": "generate_completion"
        });

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
        let head_message_id = match handle_completion_response(connection).await {
            Ok(id) => id,
            Err(e) => {
                pb.finish_and_clear();
                println!("{}: {}", "Error getting completion".red(), e);
                continue;
            }
        };

        // Get the response message using the head
        let message_request = json!({
            "type": "get_message",
            "message_id": head_message_id
        });

        // Send the message request to the actor
        connection
            .send(ManagementCommand::RequestActorMessage {
                id: actor_id_parsed.clone(),
                data: serde_json::to_vec(&message_request)
                    .context("Failed to serialize message request")?,
            })
            .await
            .context("Failed to send message request to actor")?;

        // Wait for the message response
        let response_msg = match handle_message_response(connection).await {
            Ok(msg) => msg,
            Err(e) => {
                pb.finish_and_clear();
                println!("{}: {}", "Error getting message".red(), e);
                continue;
            }
        };

        // Stop the progress bar
        pb.finish_and_clear();

        // Parse and display the assistant's response
        let response_value: serde_json::Value =
            serde_json::from_slice(&response_msg).context("Failed to parse message response")?;

        if let Some(message) = response_value.get("message").and_then(|m| m.get("message")) {
            let response_str = if let Some(content) = message.get("content") {
                if let Some(data) = content.get("data") {
                    if let Some(text) = data.as_str() {
                        text.to_string()
                    } else {
                        "Error: Could not parse assistant response text".to_string()
                    }
                } else {
                    "Error: Could not parse content data".to_string()
                }
            } else {
                "Error: Could not find content in message".to_string()
            };

            // Print the assistant's response
            term.write_str("\n")?;
            term.write_str(&format!("{} ", assistant_style.apply_to("Claude:")))?;
            term.write_str(&response_str)?;
            term.write_str("\n\n")?;
        } else {
            term.write_str("\n")?;
            term.write_str(&format!("{} ", assistant_style.apply_to("Claude:")))?;
            term.write_str("Sorry, I couldn't generate a response.")?;
            term.write_str("\n\n")?;
        }
    }

    Ok(())
}

/// Helper function to handle completion response
async fn handle_completion_response(connection: &mut TheaterConnection) -> Result<String> {
    loop {
        let resp = connection.receive().await?;
        if let ManagementResponse::RequestedMessage { message, .. } = resp {
            let response_value: serde_json::Value =
                serde_json::from_slice(&message).context("Failed to parse completion response")?;

            if let Some(head) = response_value.get("head").and_then(|h| h.as_str()) {
                return Ok(head.to_string());
            } else {
                return Err(anyhow::anyhow!("Could not extract head message ID"));
            }
        }
    }
}

/// Helper function to handle message response
async fn handle_message_response(connection: &mut TheaterConnection) -> Result<Vec<u8>> {
    loop {
        let resp = connection.receive().await?;
        if let ManagementResponse::RequestedMessage { message, .. } = resp {
            return Ok(message);
        }
    }
}

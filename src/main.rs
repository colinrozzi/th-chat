use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use console::{Style, Term};
use genai_types::{Message, MessageContent};
use indicatif::{ProgressBar, ProgressStyle};
use serde_json::json;
use std::env;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::time::Duration;
use theater::client::TheaterConnection;
use theater::theater_server::{ManagementCommand, ManagementResponse};

/// Command line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Address of the Theater server
    #[clap(long, env = "THEATER_SERVER_ADDRESS", default_value = "127.0.0.1:9000")]
    server: String,

    /// Model to use
    #[clap(long, env = "THEATER_CHAT_MODEL", default_value = "claude-3-5-sonnet-20240307")]
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
    println!("{}", "Type your messages (Ctrl+C to exit, /help for commands)".cyan());
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
async fn start_chat_state_actor(
    connection: &mut TheaterConnection,
    args: &Args,
) -> Result<String> {
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
            break id;
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
        .request(&actor_id, &serde_json::to_vec(&settings)?)
        .await
        .context("Failed to update actor settings")?;

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
                    println!("{}", "Unknown command. Type /help for available commands.".yellow());
                    continue;
                }
            }
        }

        // Skip empty messages
        if input.is_empty() {
            continue;
        }

        // Create and send message to actor
        let message = Message {
            role: "user".to_string(),
            content: MessageContent::String {
                format: None,
                data: input.to_string(),
            },
        };

        let add_message_request = json!({
            "type": "add_message",
            "message": message
        });

        // Send the message to the actor
        connection
            .request(actor_id, &serde_json::to_vec(&add_message_request)?)
            .await?;

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
        let head_response = connection
            .request(actor_id, &serde_json::to_vec(&generate_completion_request)?)
            .await?;

        // Parse the response to get the head message ID
        let head_response_parsed: serde_json::Value = serde_json::from_slice(&head_response)?;
        let head_message_id = head_response_parsed
            .get("head")
            .and_then(|h| h.as_str())
            .context("Failed to get head message ID")?;

        // Get the response message using the head
        let message_request = json!({
            "type": "get_message",
            "message_id": head_message_id
        });

        // Send the message request to the actor
        let message_response = connection
            .request(actor_id, &serde_json::to_vec(&message_request)?)
            .await?;

        // Stop the progress bar
        pb.finish_and_clear();

        // Parse and display the assistant's response
        let response_parsed: serde_json::Value = serde_json::from_slice(&message_response)?;
        if let Some(message) = response_parsed.get("message").and_then(|m| m.get("message")) {
            if let Some(content) = message.get("content") {
                let assistant_response = match content {
                    serde_json::Value::String(s) => s.to_string(),
                    serde_json::Value::Object(obj) => {
                        if let Some(serde_json::Value::String(s)) = obj.get("data") {
                            s.to_string()
                        } else {
                            "Error: Could not parse assistant response".to_string()
                        }
                    }
                    _ => "Error: Could not parse assistant response".to_string(),
                };

                // Print the assistant's response
                term.write_str("\n")?;
                term.write_str(&format!("{} ", assistant_style.apply_to("Claude:")))?;
                term.write_str(&assistant_response)?;
                term.write_str("\n\n")?;
            }
        }
    }

    Ok(())
}

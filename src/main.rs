use anyhow::{Context, Result};
use clap::Parser;
use colored::*;
use console::{Style, Term};
use genai_types::{messages::Role, Message, MessageContent};
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

    /// Debug mode to print all responses
    #[clap(long, default_value = "false")]
    debug: bool,
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
    match start_chat_state_actor(&mut connection, &args).await {
        Ok(actor_id) => {
            println!("{}", "Chat-state actor started".green());

            // Print welcome message
            println!("\n{}", "🎭 Theater Chat".bright_blue().bold());
            println!(
                "{}",
                "Type your messages (Ctrl+C to exit, /help for commands)".cyan()
            );
            println!();

            // Enter REPL loop
            run_chat_loop(&mut connection, &actor_id, args.debug).await?;
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
            "temperature": null,
            "max_tokens": 65535,
            "system_prompt": args.system_prompt.clone(),
            "title": "CLI Chat",
            "mcp_servers": []
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

/// Run the chat loop
async fn run_chat_loop(
    connection: &mut TheaterConnection,
    actor_id: &str,
    debug: bool,
) -> Result<()> {
    let term = Term::stdout();
    let user_style = Style::new().cyan().bold();
    let assistant_style = Style::new().green();
    let debug_style = Style::new().yellow().dim();

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

        // Debug the raw JSON and bytes we're sending
        if debug {
            println!("Raw JSON request: {}", add_message_request);
            let bytes = serde_json::to_vec(&add_message_request)
                .context("Failed to serialize for debug")?;
            println!(
                "First 50 bytes: {:?}",
                &bytes[..std::cmp::min(50, bytes.len())]
            );
        }

        if debug {
            println!(
                "{}",
                debug_style.apply_to(format!(
                    "DEBUG - Sending message request: {}",
                    add_message_request
                ))
            );
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

            if debug {
                println!(
                    "{}",
                    debug_style.apply_to(format!("DEBUG - Received response: {:?}", resp))
                );
            }

            match resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    if debug {
                        println!(
                            "RequestedMessage response: {}",
                            String::from_utf8_lossy(&message)
                        );
                    }
                    break;
                }
                ManagementResponse::Error { error } => {
                    println!("Error from actor: {:?}", error);
                    // Provide detailed debugging information
                    println!("Error details: {:?}", error);
                    continue;
                }
                _ => {
                    if debug {
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

        if debug {
            println!(
                "{}",
                debug_style.apply_to(format!(
                    "DEBUG - Sending completion request: {}",
                    generate_completion_request
                ))
            );
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

        // Wait for the completion response and print everything we get
        let mut head_message_id = None;
        let mut error_occurred = false;

        loop {
            let resp = connection.receive().await?;

            if debug {
                println!(
                    "{}",
                    debug_style
                        .apply_to(format!("DEBUG - Received completion response: {:?}", resp))
                );
            }

            match &resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    match serde_json::from_slice::<serde_json::Value>(message) {
                        Ok(response_value) => {
                            if let Some(head) = response_value.get("head").and_then(|h| h.as_str())
                            {
                                head_message_id = Some(head.to_string());
                                break;
                            } else if let Some(error) = response_value.get("error") {
                                pb.finish_and_clear();
                                println!("Error from actor: {}", error);
                                error_occurred = true;
                                break;
                            } else {
                                if debug {
                                    println!("Full response: {}", response_value);
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
                    // Just print and continue
                    if debug {
                        println!("Unexpected response: {:?}", resp);
                    }
                }
            }
        }

        if error_occurred {
            continue;
        }

        // If we didn't get a head message ID, continue to next input
        let head_message_id = match head_message_id {
            Some(id) => id,
            None => {
                pb.finish_and_clear();
                println!("{}", "Failed to get head message ID".red());
                continue;
            }
        };

        // Get the response message using the head
        let message_request = json!({
            "type": "get_message",
            "message_id": head_message_id
        });

        if debug {
            println!(
                "{}",
                debug_style.apply_to(format!(
                    "DEBUG - Sending message request: {}",
                    message_request
                ))
            );
        }

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
        let mut assistant_response: Option<String> = None;
        let mut error_received = false;

        while !error_received && assistant_response.is_none() {
            let resp = connection.receive().await?;

            if debug {
                println!(
                    "{}",
                    debug_style.apply_to(format!("DEBUG - Received message response: {:?}", resp))
                );
            }

            match &resp {
                ManagementResponse::RequestedMessage { message, .. } => {
                    match serde_json::from_slice::<serde_json::Value>(message) {
                        Ok(response_value) => {
                            if let Some(message_obj) =
                                response_value.get("message").and_then(|m| m.get("message"))
                            {
                                if let Some(content) = message_obj.get("content") {
                                    if let Some(data) = content.get("data") {
                                        if let Some(text) = data.as_str() {
                                            assistant_response = Some(text.to_string());
                                        } else {
                                            println!("Non-string data: {:?}", data);
                                        }
                                    } else {
                                        println!("No data field in content: {:?}", content);
                                    }
                                } else {
                                    println!("No content field in message: {:?}", message_obj);
                                }
                            } else if let Some(error) = response_value.get("error") {
                                pb.finish_and_clear();
                                println!("Error in response: {}", error);
                                error_received = true;
                            } else {
                                // Print the full response for debugging
                                if debug {
                                    println!("Unexpected response format: {}", response_value);
                                }
                            }
                        }
                        Err(e) => {
                            pb.finish_and_clear();
                            println!("Error parsing message response: {}", e);
                            println!("Raw response: {}", String::from_utf8_lossy(message));
                            error_received = true;
                        }
                    }
                }
                ManagementResponse::Error { error } => {
                    pb.finish_and_clear();
                    println!("Error from actor: {:?}", error);
                    error_received = true;
                }
                _ => {
                    // Just print and continue
                    if debug {
                        println!("Unexpected response: {:?}", resp);
                    }
                }
            }
        }

        // Stop the progress bar
        pb.finish_and_clear();

        if error_received {
            continue;
        }

        // Print the assistant's response
        if let Some(response_text) = assistant_response {
            term.write_str("\n")?;
            term.write_str(&format!("{} ", assistant_style.apply_to("Assistant:")))?;
            term.write_str(&response_text)?;
            term.write_str("\n\n")?;
        } else {
            term.write_str("\n")?;
            term.write_str(&format!("{} ", assistant_style.apply_to("Assistant:")))?;
            term.write_str("Sorry, I couldn't generate a response.")?;
            term.write_str("\n\n")?;
        }
    }

    Ok(())
}

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::fs::OpenOptions;
use std::io;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

mod app;
mod chat;
mod config;
mod config_manager;
mod directory;
mod persistence;
mod session_manager;
mod ui;

use app::App;
use config::{Args, Command, CompatibleArgs, SessionAction};
use config_manager::ConversationConfig;
use config_manager::{ConfigLoadOptions, ConfigManager};
use directory::ThChatDirectory;
use directory::{create_global_th_chat_dir, create_local_th_chat_dir};
use session_manager::{SessionInfo, SessionManager};
use uuid;

/// Extended arguments that include loaded configuration
#[derive(Debug, Clone)]
struct ExtendedArgs {
    pub server: String,
    pub debug: bool,
    pub no_session: bool,
    pub clear_session: bool,
    pub session: Option<String>,
    pub use_default_session: bool,
    pub config: ConversationConfig,
    pub sessions_directory: Option<ThChatDirectory>,
}

impl ExtendedArgs {
    /// Convert to a compatible Args structure for existing code
    pub fn to_compatible_args(&self) -> CompatibleArgs {
        // Convert MCP servers to the old format (JSON file path)
        // For now, we'll write the MCP config to a temporary location
        // This maintains full compatibility with the existing chat-state actor
        let mcp_config_path = if !self.config.mcp_servers.is_empty() {
            // We could write to a temp file, but for now let's use None
            // and handle MCP servers through the settings update instead
            None
        } else {
            None
        };
        CompatibleArgs {
            server: self.server.clone(),
            model: self.config.model_config.model.clone(),
            provider: self.config.model_config.provider.clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            system_prompt: self.config.system_prompt.clone(),
            title: self.config.title.clone(),
            debug: self.debug,
            mcp_config: mcp_config_path,
            no_session: self.no_session,
            session_dir: self
                .sessions_directory
                .as_ref()
                .map(|d| d.sessions_dir.to_string_lossy().to_string()),
            clear_session: self.clear_session,
        }
    }
}

fn setup_logging() -> Result<()> {
    // Create logs directory if it doesn't exist
    std::fs::create_dir_all("logs")?;

    // overwrite log file if it exists
    let log_file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("logs/th-chat.log")?;

    // Set up file layer
    let file_layer = fmt::layer()
        .with_writer(log_file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true);

    // Initialize tracing subscriber with both layers
    tracing_subscriber::registry()
        .with(file_layer)
        .with(tracing_subscriber::filter::LevelFilter::DEBUG)
        .init();

    Ok(())
}

/// Auto-initialize .th-chat directory if it doesn't exist
/// This provides a seamless first-run experience
async fn auto_initialize_th_chat() -> Result<ThChatDirectory> {
    info!("Auto-initializing .th-chat directory for first-time setup");

    // Try local directory first, then fall back to global
    let directory = match create_local_th_chat_dir() {
        Ok(dir) => {
            info!(
                "Created local .th-chat directory at: {}",
                dir.root.display()
            );
            dir
        }
        Err(_) => {
            info!("Local directory creation failed, trying global directory");
            create_global_th_chat_dir()
                .context("Failed to create both local and global .th-chat directories")?
        }
    };

    // Initialize with default configuration
    let config_manager = ConfigManager::new();
    config_manager
        .create_default_config(&directory)
        .context("Failed to create default configuration")?;

    info!("✅ Auto-initialization complete! Created directories and default configuration.");

    Ok(directory)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging first
    setup_logging()?;
    info!("th-chat starting up");

    let args = Args::parse();
    debug!("Parsed args: {:?}", args);

    // Handle management commands first
    if let Some(command) = &args.command {
        return handle_command(command).await;
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run
    let app = App::new(args.debug);
    let res = run_app(&mut terminal, app, args).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{:?}", err);
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut app: App,
    args: Args,
) -> Result<()> {
    // Load configuration using new system
    let config_manager = ConfigManager::new();
    let config_options = ConfigLoadOptions {
        config_file: args.config.clone(),
        preset: args.preset.clone(),
    };

    let (conversation_config, config_source) = config_manager
        .load_config(&config_options)
        .context("Failed to load configuration")?;

    info!("Using configuration from: {}", config_source);
    debug!("Loaded configuration: {:?}", conversation_config);

    // Set up session management (auto-initialize if needed)
    let sessions_dir = match config_manager.get_sessions_directory() {
        Some(dir) => dir.sessions_dir.clone(),
        None => {
            info!("No .th-chat directory found, auto-initializing...");
            let _directory = auto_initialize_th_chat()
                .await
                .context("Failed to auto-initialize .th-chat directory")?;
            let config_manager = ConfigManager::new(); // Refresh after initialization
            config_manager
                .get_sessions_directory()
                .context("Failed to find sessions directory after initialization")?
                .sessions_dir
                .clone()
        }
    };

    let session_manager = SessionManager::new(sessions_dir)?;

    // Resolve which session to use
    let session_name = session_manager
        .resolve_session_name_with_default(args.session.as_deref(), args.use_default_session);
    info!("Using session: {}", session_name);

    // Load or create session
    let mut session_data = if session_manager.session_exists(&session_name) {
        let existing_session = session_manager.load_session(&session_name)?;
        info!(
            "Loaded existing session '{}' - conversation_id: {}, messages: {}",
            existing_session.name, existing_session.conversation_id, existing_session.message_count
        );
        existing_session
    } else {
        info!("Creating new session '{}'", session_name);
        // We'll get the actual IDs from the chat-state actor after it starts
        let placeholder_conversation_id = uuid::Uuid::new_v4().to_string();
        let placeholder_store_id = uuid::Uuid::new_v4().to_string();

        let mut new_session = session_manager::SessionData::new(
            session_name.clone(),
            placeholder_conversation_id,
            placeholder_store_id,
        );

        // Associate with preset if one was used in config loading
        if let Some(preset_name) = &args.preset {
            new_session = new_session.with_preset(preset_name.clone());
        }

        new_session
    };

    // Create an extended args struct with the loaded configuration
    let extended_args = ExtendedArgs {
        server: args.server.clone(),
        debug: args.debug,
        no_session: args.no_session,
        clear_session: args.clear_session,
        session: Some(session_name.clone()),
        use_default_session: args.use_default_session,
        config: conversation_config,
        sessions_directory: config_manager.get_sessions_directory().cloned(),
    };

    // Convert to compatible format for existing code
    let compat_args = extended_args.to_compatible_args();
    info!("Starting run_app with server: {}", compat_args.server);

    // Handle session clearing if requested
    if compat_args.clear_session {
        info!("Clearing session '{}'", session_name);
        if session_manager.session_exists(&session_name)
            && session_name != SessionManager::default_session_name()
        {
            session_manager.delete_session(&session_name)?;
            info!("Session '{}' cleared successfully", session_name);

            // Create a new session
            let new_conversation_id = uuid::Uuid::new_v4().to_string();
            let new_store_id = uuid::Uuid::new_v4().to_string();
            session_data = session_manager::SessionData::new(
                session_name.clone(),
                new_conversation_id,
                new_store_id,
            );
        }
    }

    // Initialize loading steps
    app.initialize_loading_steps();

    // Step 0: Initialize
    let init_message =
        if session_manager.session_exists(&session_name) && !compat_args.clear_session {
            format!("Resuming session '{}'...", session_name)
        } else {
            format!("Initializing new session '{}'...", session_name)
        };
    app.start_loading_step(0, Some(init_message));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Small delay to show the initialization step
    //    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Step 1: Connect to server
    app.start_loading_step(
        1,
        Some(format!("Connecting to Theater server at {}", args.server)),
    );
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    info!("Connecting to Theater server...");
    let mut connection = match chat::ChatManager::connect_to_server(&compat_args).await {
        Ok(conn) => {
            info!("Connected to Theater server successfully");
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
            conn
        }
        Err(e) => {
            error!("Failed to connect to server: {:?}", e);
            app.fail_current_step(format!("Connection failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

            // Give user time to see the error
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 2: Start actor
    app.start_loading_step(
        2,
        Some(format!(
            "Starting chat-state actor for session '{}'...",
            session_name
        )),
    );
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    info!("Starting chat-state actor...");
    let actor_id = match chat::ChatManager::start_actor_with_session(
        &mut connection,
        &compat_args,
        Some(&session_data.to_persistence_session_data()),
    )
    .await
    {
        Ok(id) => {
            info!("Actor started successfully: {:?}", id);
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
            id
        }
        Err(e) => {
            error!("Failed to start actor: {:?}", e);
            app.fail_current_step(format!("Actor initialization failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 3: Open channel
    app.start_loading_step(
        3,
        Some(format!(
            "Opening channel to actor {}",
            &actor_id.to_string()[..8]
        )),
    );
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    info!("Opening channel to actor...");
    let mut chat_manager = match chat::ChatManager::open_channel_with_config(
        connection,
        actor_id,
        &compat_args,
        Some(&extended_args.config),
    )
    .await
    {
        Ok(manager) => {
            info!("Channel opened successfully");
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
            manager
        }
        Err(e) => {
            error!("Failed to open channel: {:?}", e);
            app.fail_current_step(format!("Channel setup failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 4: Get actual conversation metadata and update session
    app.start_loading_step(4, Some("Retrieving conversation metadata...".to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    match chat_manager.get_metadata().await {
        Ok((conversation_id, store_id)) => {
            // Update session with actual IDs from the actor
            session_data.conversation_id = conversation_id;
            session_data.store_id = store_id;
            session_data.update_access_time();

            // Save the updated session
            session_manager.save_session(&session_data)?;

            info!("Session metadata updated and saved");
            app.complete_current_step();
        }
        Err(e) => {
            warn!("Failed to get metadata for session: {}", e);
            app.fail_current_step(format!("Metadata retrieval failed: {}", e));
        }
    }
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Step 5: Sync conversation history (if existing session)
    if session_manager.session_exists(&session_name) && !compat_args.clear_session {
        app.start_loading_step(5, Some("Syncing conversation history...".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

        match app.sync_conversation_history(&chat_manager).await {
            Ok(_) => {
                info!("Conversation history synced successfully");
                app.complete_current_step();
            }
            Err(e) => {
                warn!("Failed to sync conversation history: {}", e);
                app.fail_current_step(format!("History sync failed: {}", e));
            }
        }
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    } else {
        // Skip history sync for new sessions
        app.start_loading_step(5, Some("Skipping history sync (new session)".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    }

    // Step 6: Prepare chat interface
    app.start_loading_step(6, Some("Preparing chat interface...".to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Final boot completion message
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Finish loading
    app.finish_loading();
    info!("Application ready for session '{}'", session_name);

    // Start main application loop with session context
    info!("Starting main application loop");
    let result = run_chat_session(
        terminal,
        &mut app,
        &mut chat_manager,
        &compat_args,
        &session_manager,
        &mut session_data,
    )
    .await;

    match &result {
        Ok(_) => info!("Application loop completed successfully"),
        Err(e) => error!("Application loop failed: {:?}", e),
    }

    result
}

/// Main chat session loop with session awareness
async fn run_chat_session(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    chat_manager: &mut chat::ChatManager,
    args: &CompatibleArgs,
    session_manager: &SessionManager,
    session_data: &mut session_manager::SessionData,
) -> Result<()> {
    info!("Starting chat session loop for '{}'", session_data.name);

    // Enhanced app.run that includes session management
    let result = app
        .run_with_session_context(terminal, chat_manager, args, session_manager, session_data)
        .await;

    // Save final session state before exiting
    if let Err(e) = session_manager.save_session(session_data) {
        warn!("Failed to save final session state: {}", e);
    } else {
        info!("Final session state saved for '{}'", session_data.name);
    }

    result
}

/// Handle session management commands
async fn handle_session_command(action: &SessionAction) -> Result<()> {
    let config_manager = ConfigManager::new();
    let sessions_dir = match config_manager.get_sessions_directory() {
        Some(dir) => dir.sessions_dir.clone(),
        None => {
            info!("No .th-chat directory found for session command, auto-initializing...");
            let _directory = auto_initialize_th_chat()
                .await
                .context("Failed to auto-initialize .th-chat directory")?;
            let config_manager = ConfigManager::new(); // Refresh after initialization
            config_manager
                .get_sessions_directory()
                .context("Failed to find sessions directory after initialization")?
                .sessions_dir
                .clone()
        }
    };

    let session_manager = SessionManager::new(sessions_dir)?;

    match action {
        SessionAction::List { detailed } => {
            let sessions = session_manager.list_sessions()?;
            if sessions.is_empty() {
                println!("No sessions found.");
            } else {
                println!("Available sessions:");
                for session in sessions {
                    if *detailed {
                        print_session_detailed(&session);
                    } else {
                        print_session_brief(&session);
                    }
                }
            }
        }

        SessionAction::New {
            name,
            description,
            preset,
        } => {
            // For now, we'll create a placeholder session. In a real implementation,
            // this would start a new conversation and get the IDs from the chat-state actor
            let conversation_id = uuid::Uuid::new_v4().to_string();
            let store_id = uuid::Uuid::new_v4().to_string();

            let session = session_manager.create_session(
                name,
                conversation_id,
                store_id,
                description.clone(),
                preset.clone(),
            )?;

            println!("✅ Created new session '{}'", session.name);
            if let Some(desc) = &session.description {
                println!("   Description: {}", desc);
            }
            if let Some(preset_name) = &session.config_preset {
                println!("   Preset: {}", preset_name);
            }
        }

        SessionAction::Info { name } => match session_manager.load_session(name) {
            Ok(session) => {
                print_session_info(&session);
            }
            Err(_) => {
                println!("❌ Session '{}' not found", name);
            }
        },

        SessionAction::Delete { name, force } => {
            if name == SessionManager::default_session_name() {
                println!("❌ Cannot delete the default session");
                return Ok(());
            }

            if !session_manager.session_exists(name) {
                println!("❌ Session '{}' not found", name);
                return Ok(());
            }

            if !force {
                print!(
                    "Are you sure you want to delete session '{}'? (y/N): ",
                    name
                );
                std::io::Write::flush(&mut std::io::stdout())?;

                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;

                if !input.trim().to_lowercase().starts_with('y') {
                    println!("Cancelled.");
                    return Ok(());
                }
            }

            session_manager.delete_session(name)?;
            println!("🗑️  Deleted session '{}'", name);
        }

        SessionAction::Rename { old_name, new_name } => {
            if !session_manager.session_exists(old_name) {
                println!("❌ Session '{}' not found", old_name);
                return Ok(());
            }

            if session_manager.session_exists(new_name) {
                println!("❌ Session '{}' already exists", new_name);
                return Ok(());
            }

            session_manager.rename_session(old_name, new_name)?;
            println!("✅ Renamed session '{}' to '{}'", old_name, new_name);
        }

        SessionAction::Clean {
            older_than,
            dry_run,
        } => {
            let days = if let Some(duration_str) = older_than {
                parse_duration_days(duration_str)?
            } else {
                30 // Default to 30 days
            };

            let deleted = session_manager.clean_sessions(Some(days), *dry_run)?;

            if *dry_run {
                if deleted.is_empty() {
                    println!("No sessions would be deleted.");
                } else {
                    println!("Would delete {} session(s):", deleted.len());
                    for session_name in deleted {
                        println!("  - {}", session_name);
                    }
                }
            } else {
                if deleted.is_empty() {
                    println!("No sessions were deleted.");
                } else {
                    println!("🧹 Cleaned {} session(s)", deleted.len());
                }
            }
        }
    }

    Ok(())
}

/// Handle management commands
async fn handle_command(command: &Command) -> Result<()> {
    match command {
        Command::Init { global } => {
            let directory = if *global {
                info!("Creating global .th-chat directory");
                create_global_th_chat_dir()?
            } else {
                info!("Creating local .th-chat directory");
                create_local_th_chat_dir()?
            };

            let config_manager = ConfigManager::new();
            config_manager.create_default_config(&directory)?;

            println!(
                "✅ Initialized .th-chat directory at: {}",
                directory.root.display()
            );
            println!("📁 Created directories:");
            println!("   - config.json (main configuration)");
            println!("   - sessions/ (conversation sessions)");
            println!("   - presets/ (configuration presets)");
            println!("   - mcp/ (MCP server configurations)");
            println!("📄 Created example presets:");
            println!("   - coding.json");
            println!("   - research.json");
            println!();
            println!("🚀 You can now run 'th-chat' to start chatting!");
        }

        Command::Presets => {
            let config_manager = ConfigManager::new();
            let presets = config_manager.list_presets()?;

            if presets.is_empty() {
                println!("No presets found. Run 'th-chat init' to create example presets.");
            } else {
                println!("Available presets:");
                for (name, source) in presets {
                    println!("  {} ({})", name, source);
                }
            }
        }

        Command::Sessions { action } => {
            handle_session_command(action).await?;
        }

        Command::Config { preset } => {
            let config_manager = ConfigManager::new();
            let options = ConfigLoadOptions {
                config_file: None,
                preset: preset.clone(),
            };

            let (config, source) = config_manager.load_config(&options)?;

            println!("Configuration source: {}", source);
            println!();
            println!("{}", serde_json::to_string_pretty(&config)?);
        }
    }

    Ok(())
}

/// Print brief session information
fn print_session_brief(session: &SessionInfo) {
    let age = format_age(session.age_hours());
    let preset_info = session
        .config_preset
        .as_ref()
        .map(|p| format!(" [{}]", p))
        .unwrap_or_default();

    println!(
        "  {} ({} messages, {}){}",
        session.name, session.message_count, age, preset_info
    );
}

/// Print detailed session information
fn print_session_detailed(session: &SessionInfo) {
    println!("  📝 {}", session.name);
    if let Some(desc) = &session.description {
        println!("     Description: {}", desc);
    }
    println!("     Messages: {}", session.message_count);
    println!("     Last used: {}", format_age(session.age_hours()));
    if let Some(preset) = &session.config_preset {
        println!("     Preset: {}", preset);
    }
    println!();
}

/// Print complete session information
fn print_session_info(session: &session_manager::SessionData) {
    println!("📝 Session: {}", session.name);
    if let Some(desc) = &session.description {
        println!("   Description: {}", desc);
    }
    println!("   Created: {}", format_timestamp(session.created_at));
    println!(
        "   Last accessed: {}",
        format_timestamp(session.last_accessed)
    );
    println!("   Messages: {}", session.message_count);
    if let Some(preset) = &session.config_preset {
        println!("   Config preset: {}", preset);
    }
    println!("   Conversation ID: {}", session.conversation_id);
    println!("   Store ID: {}", session.store_id);
}

/// Format age in hours to human-readable string
fn format_age(hours: u64) -> String {
    if hours == 0 {
        "just now".to_string()
    } else if hours < 24 {
        format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
    } else {
        let days = hours / 24;
        format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
    }
}

/// Format Unix timestamp to human-readable string
fn format_timestamp(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let datetime = UNIX_EPOCH + std::time::Duration::from_secs(timestamp);

    // Simple formatting - in a real app you might want to use chrono
    match SystemTime::now().duration_since(datetime) {
        Ok(duration) => {
            let hours = duration.as_secs() / 3600;
            format_age(hours)
        }
        Err(_) => "in the future".to_string(),
    }
}

/// Parse duration string like "30d", "7d", "24h" to days
fn parse_duration_days(duration_str: &str) -> Result<u64> {
    let duration_str = duration_str.trim().to_lowercase();

    if let Some(num_str) = duration_str.strip_suffix('d') {
        let days: u64 = num_str
            .parse()
            .with_context(|| format!("Invalid duration: {}", duration_str))?;
        Ok(days)
    } else if let Some(num_str) = duration_str.strip_suffix('h') {
        let hours: u64 = num_str
            .parse()
            .with_context(|| format!("Invalid duration: {}", duration_str))?;
        Ok(hours / 24) // Convert hours to days (rounded down)
    } else {
        Err(anyhow::anyhow!(
            "Invalid duration format. Use format like '30d' or '24h'"
        ))
    }
}

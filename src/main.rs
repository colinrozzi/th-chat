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
mod ui;

use app::App;
use config::{Args, Command, CompatibleArgs, CHAT_STATE_ACTOR_MANIFEST};
use config_manager::{ConfigManager, ConfigLoadOptions};
use directory::{create_local_th_chat_dir, create_global_th_chat_dir};
use persistence::{SessionData, session_exists, load_session, save_session, clear_session};
use config_manager::ConversationConfig;
use directory::ThChatDirectory;

/// Extended arguments that include loaded configuration
#[derive(Debug, Clone)]
struct ExtendedArgs {
    pub server: String,
    pub debug: bool,
    pub no_session: bool,
    pub clear_session: bool,
    pub config: ConversationConfig,
    pub sessions_directory: Option<ThChatDirectory>,
}

impl ExtendedArgs {
    /// Convert to a compatible Args structure for existing code
    pub fn to_compatible_args(&self) -> CompatibleArgs {
        CompatibleArgs {
            server: self.server.clone(),
            model: self.config.model_config.model.clone(),
            provider: self.config.model_config.provider.clone(),
            temperature: self.config.temperature,
            max_tokens: self.config.max_tokens,
            system_prompt: self.config.system_prompt.clone(),
            title: self.config.title.clone(),
            debug: self.debug,
            mcp_config: if self.config.mcp_servers.is_empty() {
                None
            } else {
                // For now, we'll handle MCP servers differently
                // This is a placeholder for compatibility
                None
            },
            no_session: self.no_session,
            session_dir: self.sessions_directory.as_ref().map(|d| d.sessions_dir.to_string_lossy().to_string()),
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
    
    let (conversation_config, config_source) = config_manager.load_config(&config_options)
        .context("Failed to load configuration")?;
    
    info!("Using configuration from: {}", config_source);
    debug!("Loaded configuration: {:?}", conversation_config);
    
    // Create an extended args struct with the loaded configuration
    let extended_args = ExtendedArgs {
        server: args.server.clone(),
        debug: args.debug,
        no_session: args.no_session,
        clear_session: args.clear_session,
        config: conversation_config,
        sessions_directory: config_manager.get_sessions_directory().cloned(),
    };
    
    // Convert to compatible format for existing code
    let compat_args = extended_args.to_compatible_args();
    info!("Starting run_app with server: {}", compat_args.server);

    // Handle session clearing if requested
    let session_dir = compat_args.session_dir.as_ref().map(|s| std::path::Path::new(s));
    if compat_args.clear_session {
        match clear_session(session_dir) {
            Ok(_) => info!("Existing session cleared successfully"),
            Err(e) => warn!("Failed to clear session: {}", e),
        }
    }

    // Check for existing session first
    let existing_session = if !compat_args.no_session && !compat_args.clear_session && session_exists(session_dir) {
        match load_session(session_dir) {
            Ok(session) => {
                info!("Found existing session - conversation_id: {}, store_id: {}", 
                     session.conversation_id, session.store_id);
                Some(session)
            }
            Err(e) => {
                warn!("Failed to load existing session: {}, starting new session", e);
                None
            }
        }
    } else {
        info!("No existing session found or session persistence disabled");
        None
    };

    // Step 0: Initialize (already set as InProgress in App::new())
    let init_message = if existing_session.is_some() {
        "Resuming existing session..."
    } else {
        "Initializing new session..."
    };
    app.start_loading_step(0, Some(init_message.to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    
    // Small delay to show the initialization step
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Step 1: Connect to server
    app.start_loading_step(1, Some(format!("Connecting to Theater server at {}", args.server)));
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

    // Step 2: Load actor manifest
    app.start_loading_step(2, Some(format!("Loading manifest: {}", 
        CHAT_STATE_ACTOR_MANIFEST.split('/').last().unwrap_or("manifest.toml"))));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    
    // Small delay to show manifest loading
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Step 3: Start actor
    app.start_loading_step(3, None);
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    
    info!("Starting chat-state actor...");
    let actor_id = match chat::ChatManager::start_actor_with_session(&mut connection, &compat_args, existing_session.as_ref()).await {
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

    // Step 4: Open channel
    app.start_loading_step(4, Some(format!("Opening channel to actor {}", &actor_id.to_string()[..8])));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    
    info!("Opening channel to actor...");
    let mut chat_manager = match chat::ChatManager::open_channel(connection, actor_id, &compat_args).await {
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

    // Step 5: Initialize MCP (if configured)
    if compat_args.mcp_config.is_some() {
        app.start_loading_step(5, Some("Starting MCP server connections".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        
        // MCP initialization happens in the open_channel step
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    } else {
        // Skip MCP step
        app.start_loading_step(5, Some("Skipping MCP servers (not configured)".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    }

    // Step 6: Save session data (if enabled and new session)
    if !args.no_session && existing_session.is_none() {
        app.start_loading_step(6, Some("Saving session data...".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        
        // Get metadata from the actor and save session
        match chat_manager.get_metadata().await {
            Ok((conversation_id, store_id)) => {
                let session_data = SessionData::new(conversation_id, store_id);
                match save_session(&session_data, session_dir) {
                    Ok(_) => {
                        info!("Session data saved successfully");
                        app.complete_current_step();
                    }
                    Err(e) => {
                        warn!("Failed to save session: {}", e);
                        app.fail_current_step(format!("Session save failed: {}", e));
                    }
                }
            }
            Err(e) => {
                warn!("Failed to get metadata for session: {}", e);
                app.fail_current_step(format!("Metadata retrieval failed: {}", e));
            }
        }
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    } else {
        // Skip session saving
        let skip_message = if args.no_session {
            "Session persistence disabled"
        } else {
            "Using existing session data"
        };
        app.start_loading_step(6, Some(skip_message.to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    }

    // Step 7: Sync conversation history (if resuming session)
    if existing_session.is_some() {
        app.start_loading_step(7, Some("Syncing conversation history...".to_string()));
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
        app.start_loading_step(7, Some("Skipping history sync (new session)".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    }

    // Step 8: Prepare chat interface
    app.start_loading_step(8, Some("Preparing chat interface...".to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;
    
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &compat_args))?;

    // Final boot completion message
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Finish loading
    app.finish_loading();
    info!("Application ready");

    // Start main application loop
    info!("Starting main application loop");
    let result = app.run(terminal, &mut chat_manager, &compat_args).await;

    match &result {
        Ok(_) => info!("Application loop completed successfully"),
        Err(e) => error!("Application loop failed: {:?}", e),
    }

    result
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
            
            println!("✅ Initialized .th-chat directory at: {}", directory.root.display());
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
        
        Command::Sessions { clean } => {
            let config_manager = ConfigManager::new();
            if let Some(directory) = config_manager.get_sessions_directory() {
                if *clean {
                    let sessions = directory.list_sessions()?;
                    if sessions.is_empty() {
                        println!("No sessions to clean.");
                    } else {
                        for session in &sessions {
                            let session_file = directory.session_file(session);
                            std::fs::remove_file(&session_file)
                                .with_context(|| format!("Failed to remove session: {}", session))?;
                        }
                        println!("🧹 Cleaned {} session(s)", sessions.len());
                    }
                } else {
                    let sessions = directory.list_sessions()?;
                    if sessions.is_empty() {
                        println!("No active sessions.");
                    } else {
                        println!("Active sessions:");
                        for session in sessions {
                            println!("  {}", session);
                        }
                    }
                }
            } else {
                println!("No .th-chat directory found. Run 'th-chat init' to initialize.");
            }
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

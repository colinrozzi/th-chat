use anyhow::Result;
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
mod persistence;
mod ui;

use app::App;
use config::{Args, CHAT_STATE_ACTOR_MANIFEST};
use persistence::{SessionData, session_exists, load_session, save_session, clear_session};

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
    info!("Starting run_app with server: {}", args.server);

    // Handle session clearing if requested
    let session_dir = args.session_dir.as_ref().map(|s| std::path::Path::new(s));
    if args.clear_session {
        match clear_session(session_dir) {
            Ok(_) => info!("Existing session cleared successfully"),
            Err(e) => warn!("Failed to clear session: {}", e),
        }
    }

    // Check for existing session first
    let existing_session = if !args.no_session && !args.clear_session && session_exists(session_dir) {
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
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    // Small delay to show the initialization step
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &args))?;

    // Step 1: Connect to server
    app.start_loading_step(1, Some(format!("Connecting to Theater server at {}", args.server)));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    info!("Connecting to Theater server...");
    let mut connection = match chat::ChatManager::connect_to_server(&args).await {
        Ok(conn) => {
            info!("Connected to Theater server successfully");
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            conn
        }
        Err(e) => {
            error!("Failed to connect to server: {:?}", e);
            app.fail_current_step(format!("Connection failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            
            // Give user time to see the error
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 2: Load actor manifest
    app.start_loading_step(2, Some(format!("Loading manifest: {}", 
        CHAT_STATE_ACTOR_MANIFEST.split('/').last().unwrap_or("manifest.toml"))));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    // Small delay to show manifest loading
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &args))?;

    // Step 3: Start actor
    app.start_loading_step(3, None);
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    info!("Starting chat-state actor...");
    let actor_id = match chat::ChatManager::start_actor_with_session(&mut connection, &args, existing_session.as_ref()).await {
        Ok(id) => {
            info!("Actor started successfully: {:?}", id);
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            id
        }
        Err(e) => {
            error!("Failed to start actor: {:?}", e);
            app.fail_current_step(format!("Actor initialization failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 4: Open channel
    app.start_loading_step(4, Some(format!("Opening channel to actor {}", &actor_id.to_string()[..8])));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    info!("Opening channel to actor...");
    let mut chat_manager = match chat::ChatManager::open_channel(connection, actor_id, &args).await {
        Ok(manager) => {
            info!("Channel opened successfully");
            app.complete_current_step();
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            manager
        }
        Err(e) => {
            error!("Failed to open channel: {:?}", e);
            app.fail_current_step(format!("Channel setup failed: {}", e));
            terminal.draw(|f| ui::render(f, &mut app, &args))?;
            
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            return Err(e);
        }
    };

    // Step 5: Initialize MCP (if configured)
    if args.mcp_config.is_some() {
        app.start_loading_step(5, Some("Starting MCP server connections".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        
        // MCP initialization happens in the open_channel step
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
    } else {
        // Skip MCP step
        app.start_loading_step(5, Some("Skipping MCP servers (not configured)".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
    }

    // Step 6: Save session data (if enabled and new session)
    if !args.no_session && existing_session.is_none() {
        app.start_loading_step(6, Some("Saving session data...".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        
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
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    } else {
        // Skip session saving
        let skip_message = if args.no_session {
            "Session persistence disabled"
        } else {
            "Using existing session data"
        };
        app.start_loading_step(6, Some(skip_message.to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
    }

    // Step 7: Sync conversation history (if resuming session)
    if existing_session.is_some() {
        app.start_loading_step(7, Some("Syncing conversation history...".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        
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
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    } else {
        // Skip history sync for new sessions
        app.start_loading_step(7, Some("Skipping history sync (new session)".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        app.complete_current_step();
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
    }

    // Step 8: Prepare chat interface
    app.start_loading_step(8, Some("Preparing chat interface...".to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    app.complete_current_step();
    terminal.draw(|f| ui::render(f, &mut app, &args))?;

    // Final boot completion message
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Finish loading
    app.finish_loading();
    info!("Application ready");

    // Start main application loop
    info!("Starting main application loop");
    let result = app.run(terminal, &mut chat_manager, &args).await;

    match &result {
        Ok(_) => info!("Application loop completed successfully"),
        Err(e) => error!("Application loop failed: {:?}", e),
    }

    result
}

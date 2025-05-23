use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::fs::OpenOptions;
use std::io;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt};

mod app;
mod chat;
mod config;
mod ui;

use app::App;
use config::{Args, LoadingState, CHAT_STATE_ACTOR_MANIFEST};

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

    // Start with loading screen
    app.set_loading_state(LoadingState::ConnectingToServer(args.server.clone()));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;

    // Step 1: Connect to server
    info!("Connecting to Theater server...");
    let mut connection = match chat::ChatManager::connect_to_server(&args).await {
        Ok(conn) => {
            info!("Connected to Theater server successfully");
            conn
        }
        Err(e) => {
            error!("Failed to connect to server: {:?}", e);
            return Err(e);
        }
    };

    // Step 2: Start actor
    app.set_loading_state(LoadingState::StartingActor(CHAT_STATE_ACTOR_MANIFEST.to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    info!("Starting chat-state actor...");
    let actor_id = match chat::ChatManager::start_actor(&mut connection, &args).await {
        Ok(id) => {
            info!("Actor started successfully: {:?}", id);
            id
        }
        Err(e) => {
            error!("Failed to start actor: {:?}", e);
            return Err(e);
        }
    };

    // Step 3: Open channel
    app.set_loading_state(LoadingState::OpeningChannel(actor_id.to_string()));
    terminal.draw(|f| ui::render(f, &mut app, &args))?;
    
    info!("Opening channel to actor...");
    let mut chat_manager = match chat::ChatManager::open_channel(connection, actor_id, &args).await {
        Ok(manager) => {
            info!("Channel opened successfully");
            manager
        }
        Err(e) => {
            error!("Failed to open channel: {:?}", e);
            return Err(e);
        }
    };

    // Step 4: Initialize MCP (if configured)
    if args.mcp_config.is_some() {
        app.set_loading_state(LoadingState::InitializingMcp("Starting MCP servers".to_string()));
        terminal.draw(|f| ui::render(f, &mut app, &args))?;
        // MCP initialization is handled in open_channel step
    }

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

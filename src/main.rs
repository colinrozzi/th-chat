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
use config::Args;

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

    // Update app connection status during initialization
    app.set_connection_status("Connecting...".to_string());
    info!("Set connection status to 'Connecting...'");

    // Initialize the chat system
    info!("Initializing chat manager...");
    let mut chat_manager = match chat::ChatManager::new(&args).await {
        Ok(manager) => {
            info!("Chat manager initialized successfully");
            manager
        }
        Err(e) => {
            error!("Failed to initialize chat manager: {:?}", e);
            return Err(e);
        }
    };

    // Update status to ready
    app.set_connection_status("Ready".to_string());
    info!("Set connection status to 'Ready'");

    // Main application loop
    info!("Starting main application loop");
    let result = app.run(terminal, &mut chat_manager, &args).await;

    match &result {
        Ok(_) => info!("Application loop completed successfully"),
        Err(e) => error!("Application loop failed: {:?}", e),
    }

    result
}

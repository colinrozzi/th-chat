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
use config::{Args, CHAT_STATE_ACTOR_MANIFEST};

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

    // Step 0: Initialize (already set as InProgress in App::new())
    app.start_loading_step(0, None);
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
    let actor_id = match chat::ChatManager::start_actor(&mut connection, &args).await {
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

    // Step 6: Prepare chat interface
    app.start_loading_step(6, None);
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

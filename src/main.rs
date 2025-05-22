use anyhow::Result;
use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

mod app;
mod chat;
mod config;
mod ui;

use app::App;
use config::Args;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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
    // Update app connection status during initialization
    app.set_connection_status("Connecting...".to_string());
    
    // Initialize the chat system
    let mut chat_manager = chat::ChatManager::new(&args).await?;
    
    // Update status to ready
    app.set_connection_status("Ready".to_string());
    
    // Main application loop
    app.run(terminal, &mut chat_manager, &args).await
}

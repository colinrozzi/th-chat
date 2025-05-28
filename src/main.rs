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
use config::{Args, Command, CompatibleArgs, SessionAction, CHAT_STATE_ACTOR_MANIFEST};
use config_manager::{ConfigManager, ConfigLoadOptions};
use directory::{create_local_th_chat_dir, create_global_th_chat_dir};
use persistence::{SessionData, session_exists, load_session, save_session, clear_session};
use session_manager::{SessionManager, SessionInfo};
use config_manager::ConversationConfig;
use directory::ThChatDirectory;

/// Extended arguments that include loaded configuration
#[derive(Debug, Clone)]
struct ExtendedArgs {
    pub server: String,
    pub debug: bool,
    pub no_session: bool,
    pub clear_session: bool,
    pub session: Option<String>,
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
    // This is a simplified version - the actual implementation would be more complex
    // For now, just show that sessions can be handled
    info!("Session handling would be implemented here");
    info!("Requested session: {:?}", args.session);
    
    // TODO: Implement full session handling in the main application flow
    Ok(())
}

/// Handle session management commands
async fn handle_session_command(action: &SessionAction) -> Result<()> {
    let config_manager = ConfigManager::new();
    let sessions_dir = match config_manager.get_sessions_directory() {
        Some(dir) => dir.sessions_dir.clone(),
        None => {
            println!("No .th-chat directory found. Run 'th-chat init' to initialize.");
            return Ok(());
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
        
        SessionAction::New { name, description, preset } => {
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
        
        SessionAction::Info { name } => {
            match session_manager.load_session(name) {
                Ok(session) => {
                    print_session_info(&session);
                }
                Err(_) => {
                    println!("❌ Session '{}' not found", name);
                }
            }
        }
        
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
                print!("Are you sure you want to delete session '{}'? (y/N): ", name);
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
        
        SessionAction::Clean { older_than, dry_run } => {
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
    let preset_info = session.config_preset.as_ref()
        .map(|p| format!(" [{}]", p))
        .unwrap_or_default();
    
    println!("  {} ({} messages, {}){}", 
             session.name, 
             session.message_count, 
             age,
             preset_info);
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
    println!("   Last accessed: {}", format_timestamp(session.last_accessed));
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
        let days: u64 = num_str.parse()
            .with_context(|| format!("Invalid duration: {}", duration_str))?;
        Ok(days)
    } else if let Some(num_str) = duration_str.strip_suffix('h') {
        let hours: u64 = num_str.parse()
            .with_context(|| format!("Invalid duration: {}", duration_str))?;
        Ok(hours / 24) // Convert hours to days (rounded down)
    } else {
        Err(anyhow::anyhow!("Invalid duration format. Use format like '30d' or '24h'"))
    }
}

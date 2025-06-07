use clap::Parser;
use ratatui;

/// Individual loading step with status
#[derive(Debug, Clone, PartialEq)]
pub struct LoadingStep {
    pub message: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,        // Not started yet
    InProgress,     // Currently running
    Success,        // Completed successfully
    Failed(String), // Failed with error message
}

impl StepStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            StepStatus::Pending => "    ",
            StepStatus::InProgress => "[WAIT]",
            StepStatus::Success => "[ OK ]",
            StepStatus::Failed(_) => "[FAIL]",
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        match self {
            StepStatus::Pending => ratatui::style::Color::DarkGray,
            StepStatus::InProgress => ratatui::style::Color::Yellow,
            StepStatus::Success => ratatui::style::Color::Green,
            StepStatus::Failed(_) => ratatui::style::Color::Red,
        }
    }
}

/// Loading states during application startup
#[derive(Debug, Clone, PartialEq)]
pub enum LoadingState {
    ConnectingToServer(String), // server address
    StartingActor(String),      // actor manifest path
    OpeningChannel(String),     // actor ID
    InitializingMcp(String),    // MCP config status
    Ready,
}

impl LoadingState {
    pub fn message(&self) -> String {
        match self {
            LoadingState::ConnectingToServer(addr) => {
                format!("Connecting to Theater server at {}", addr)
            }
            LoadingState::StartingActor(manifest) => format!(
                "Starting chat-state actor from {}",
                manifest.split('/').last().unwrap_or(manifest)
            ),
            LoadingState::OpeningChannel(actor_id) => {
                format!("Opening communication channel to actor {}", &actor_id[..8])
            }
            LoadingState::InitializingMcp(status) => {
                format!("Initializing MCP servers: {}", status)
            }
            LoadingState::Ready => "System ready".to_string(),
        }
    }
}

/// Command line arguments
#[derive(Parser, Debug, Clone)]
#[clap(author, version, about)]
pub struct Args {
    /// Address of the Theater server
    #[clap(long, env = "THEATER_SERVER_ADDRESS", default_value = "127.0.0.1:9000")]
    pub server: String,

    /// Use a specific configuration file
    #[clap(short = 'c', long, value_name = "FILE")]
    pub config: Option<std::path::PathBuf>,

    /// Use a named preset configuration
    #[clap(short = 'p', long, value_name = "PRESET")]
    pub preset: Option<String>,

    /// Debug mode to print all responses
    #[clap(short = 'd', long, default_value = "false")]
    pub debug: bool,

    /// Disable session persistence (always start a new conversation)
    #[clap(short = 'N', long, default_value = "false")]
    pub no_session: bool,

    /// Clear existing session and start fresh
    #[clap(short = 'C', long, default_value = "false")]
    pub clear_session: bool,

    /// Use specific session
    #[clap(short = 's', long, env = "TH_CHAT_SESSION")]
    pub session: Option<String>,

    /// Use the default session instead of creating a new auto-incremented session
    #[clap(short = 'U', long, default_value = "false")]
    pub use_default_session: bool,

    /// Subcommands for management operations
    #[clap(subcommand)]
    pub command: Option<Command>,
}

/// Management subcommands
#[derive(Parser, Debug, Clone)]
pub enum Command {
    /// Initialize .th-chat directory structure
    Init {
        /// Create global configuration instead of local
        #[clap(short = 'g', long)]
        global: bool,
    },
    /// List available presets
    Presets,
    /// Manage chat sessions
    Sessions {
        #[clap(subcommand)]
        action: SessionAction,
    },
    /// Show resolved configuration
    Config {
        /// Show configuration for specific preset
        #[clap(short = 'p', long)]
        preset: Option<String>,
    },
}

/// Session management subcommands
#[derive(Parser, Debug, Clone)]
pub enum SessionAction {
    /// List all sessions
    List {
        /// Show detailed information
        #[clap(short = 'l', long)]
        detailed: bool,
    },
    /// Create new session
    New {
        /// Session name
        #[clap(short = 'n', long)]
        name: String,
        /// Optional description
        #[clap(short = 'D', long)]
        description: Option<String>,
        /// Associate with config preset
        #[clap(short = 'P', long)]
        preset: Option<String>,
    },
    /// Show session information
    Info {
        /// Session name
        name: String,
    },
    /// Delete session
    Delete {
        /// Session name
        name: String,
        /// Force deletion without confirmation
        #[clap(short = 'f', long)]
        force: bool,
    },
    /// Rename session
    Rename {
        /// Current session name
        old_name: String,
        /// New session name
        new_name: String,
    },
    /// Clean old sessions
    Clean {
        /// Delete sessions older than specified duration (e.g., "30d", "7d")
        #[clap(short = 'o', long)]
        older_than: Option<String>,
        /// Show what would be deleted without actually deleting
        #[clap(short = 'r', long)]
        dry_run: bool,
    },
}

/// Compatibility structure that matches the old Args interface
#[derive(Debug, Clone)]
pub struct CompatibleArgs {
    pub server: String,
    pub model: String,
    pub provider: String,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    pub system_prompt: Option<String>,
    pub title: String,
    pub debug: bool,
    pub mcp_config: Option<String>,

    pub clear_session: bool,
}

// Chat state actor manifest path
pub const CHAT_STATE_ACTOR_MANIFEST: &str =
    "/Users/colinrozzi/work/actor-registry/chat-state/manifest.toml";

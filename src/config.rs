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
    #[clap(long, value_name = "FILE")]
    pub config: Option<std::path::PathBuf>,

    /// Use a named preset configuration
    #[clap(long, value_name = "PRESET")]
    pub preset: Option<String>,

    /// Debug mode to print all responses
    #[clap(long, default_value = "false")]
    pub debug: bool,

    /// Disable session persistence (always start a new conversation)
    #[clap(long, default_value = "false")]
    pub no_session: bool,

    /// Clear existing session and start fresh
    #[clap(long, default_value = "false")]
    pub clear_session: bool,

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
        #[clap(long)]
        global: bool,
    },
    /// List available presets
    Presets,
    /// List current sessions
    Sessions {
        /// Clean old session files
        #[clap(long)]
        clean: bool,
    },
    /// Show resolved configuration
    Config {
        /// Show configuration for specific preset
        #[clap(long)]
        preset: Option<String>,
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
    pub no_session: bool,
    pub session_dir: Option<String>,
    pub clear_session: bool,
}

// Chat state actor manifest path
pub const CHAT_STATE_ACTOR_MANIFEST: &str =
    "/Users/colinrozzi/work/actor-registry/chat-state/manifest.toml";

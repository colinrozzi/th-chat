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

    /// Model to use
    #[clap(
        long,
        env = "THEATER_CHAT_MODEL",
        default_value = "gemini-2.5-flash-preview-04-17"
    )]
    pub model: String,

    /// Provider to use
    #[clap(long, env = "THEATER_CHAT_PROVIDER", default_value = "google")]
    pub provider: String,

    /// Temperature setting (0.0 to 1.0)
    #[clap(long, env = "THEATER_CHAT_TEMPERATURE")]
    pub temperature: Option<f32>,

    /// Maximum tokens to generate
    #[clap(long, env = "THEATER_CHAT_MAX_TOKENS", default_value = "65535")]
    pub max_tokens: u32,

    /// System prompt
    #[clap(long, env = "THEATER_CHAT_SYSTEM_PROMPT")]
    pub system_prompt: Option<String>,

    /// Conversation title
    #[clap(long, env = "THEATER_CHAT_TITLE", default_value = "CLI Chat")]
    pub title: String,

    /// Debug mode to print all responses
    #[clap(long, default_value = "false")]
    pub debug: bool,

    /// Path to MCP servers configuration file (JSON)
    #[clap(long, env = "THEATER_CHAT_MCP_CONFIG")]
    pub mcp_config: Option<String>,
}

// Chat state actor manifest path
pub const CHAT_STATE_ACTOR_MANIFEST: &str =
    "/Users/colinrozzi/work/actor-registry/chat-state/manifest.toml";

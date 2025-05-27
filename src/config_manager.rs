use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::directory::{ThChatDirectory, find_th_chat_dir, get_global_th_chat_dir};

/// Full conversation settings that match the chat-state actor's ConversationSettings
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationConfig {
    /// Model configuration
    pub model_config: ModelConfig,
    
    /// Temperature setting (0.0 to 1.0)
    pub temperature: Option<f32>,
    
    /// Maximum tokens to generate
    pub max_tokens: u32,
    
    /// System prompt to use
    pub system_prompt: Option<String>,
    
    /// Title of the conversation
    pub title: String,
    
    /// MCP servers configuration
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelConfig {
    /// Model name (e.g., "gemini-2.5-flash-preview-04-17")
    pub model: String,
    
    /// Provider name (e.g., "google", "anthropic")
    pub provider: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpServerConfig {
    /// Actor ID (will be populated when server is started)
    pub actor_id: Option<String>,
    
    /// Server configuration
    pub config: McpConfig,
    
    /// Available tools (will be populated when server starts)
    pub tools: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpConfig {
    /// Command to execute
    pub command: String,
    
    /// Command arguments
    pub args: Vec<String>,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            model_config: ModelConfig {
                model: "gemini-2.5-flash-preview-04-17".to_string(),
                provider: "google".to_string(),
            },
            temperature: None,
            max_tokens: 65535,
            system_prompt: None,
            title: "CLI Chat".to_string(),
            mcp_servers: vec![],
        }
    }
}

/// Configuration source information
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Built-in default configuration
    Default,
    /// Global configuration file (~/.th-chat/config.json)
    Global(ThChatDirectory),
    /// Local project configuration (./.th-chat/config.json)
    Local(ThChatDirectory),
    /// Named preset file
    Preset { directory: ThChatDirectory, name: String },
    /// Explicit configuration file path
    File(std::path::PathBuf),
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigSource::Default => write!(f, "built-in defaults"),
            ConfigSource::Global(dir) => write!(f, "global config ({})", dir.config_file.display()),
            ConfigSource::Local(dir) => write!(f, "local config ({})", dir.config_file.display()),
            ConfigSource::Preset { directory, name } => {
                write!(f, "preset '{}' ({})", name, directory.preset_file(name).display())
            }
            ConfigSource::File(path) => write!(f, "config file ({})", path.display()),
        }
    }
}

/// Configuration manager handles loading and resolving configurations
pub struct ConfigManager {
    /// Local .th-chat directory (if found)
    local_dir: Option<ThChatDirectory>,
    
    /// Global .th-chat directory (if exists)
    global_dir: Option<ThChatDirectory>,
}

impl ConfigManager {
    /// Create a new configuration manager
    pub fn new() -> Self {
        let local_dir = find_th_chat_dir();
        let global_dir = get_global_th_chat_dir().filter(|d| d.exists());
        
        debug!("ConfigManager initialized");
        if let Some(ref local) = local_dir {
            debug!("Found local .th-chat directory: {}", local.root.display());
        }
        if let Some(ref global) = global_dir {
            debug!("Found global .th-chat directory: {}", global.root.display());
        }
        
        Self { local_dir, global_dir }
    }
    
    /// Load configuration with the specified options
    pub fn load_config(&self, options: &ConfigLoadOptions) -> Result<(ConversationConfig, ConfigSource)> {
        // Priority order:
        // 1. Explicit config file (--config)
        // 2. Named preset (--preset)
        // 3. Local project config (./.th-chat/config.json)
        // 4. Global config (~/.th-chat/config.json)
        // 5. Built-in defaults
        
        if let Some(config_file) = &options.config_file {
            info!("Loading configuration from explicit file: {}", config_file.display());
            let config = self.load_config_file(config_file)
                .with_context(|| format!("Failed to load config file: {}", config_file.display()))?;
            return Ok((config, ConfigSource::File(config_file.clone())));
        }
        
        if let Some(preset_name) = &options.preset {
            info!("Loading configuration from preset: {}", preset_name);
            return self.load_preset(preset_name);
        }
        
        // Try local config
        if let Some(ref local_dir) = self.local_dir {
            if local_dir.has_config() {
                info!("Loading local configuration: {}", local_dir.config_file.display());
                match self.load_config_file(&local_dir.config_file) {
                    Ok(config) => return Ok((config, ConfigSource::Local(local_dir.clone()))),
                    Err(e) => {
                        warn!("Failed to load local config, falling back: {}", e);
                    }
                }
            }
        }
        
        // Try global config
        if let Some(ref global_dir) = self.global_dir {
            if global_dir.has_config() {
                info!("Loading global configuration: {}", global_dir.config_file.display());
                match self.load_config_file(&global_dir.config_file) {
                    Ok(config) => return Ok((config, ConfigSource::Global(global_dir.clone()))),
                    Err(e) => {
                        warn!("Failed to load global config, falling back: {}", e);
                    }
                }
            }
        }
        
        // Fall back to defaults
        info!("Using built-in default configuration");
        Ok((ConversationConfig::default(), ConfigSource::Default))
    }
    
    /// Load a named preset
    pub fn load_preset(&self, preset_name: &str) -> Result<(ConversationConfig, ConfigSource)> {
        // Try local presets first, then global
        if let Some(ref local_dir) = self.local_dir {
            let preset_file = local_dir.preset_file(preset_name);
            if preset_file.exists() {
                debug!("Loading local preset: {}", preset_file.display());
                let config = self.load_config_file(&preset_file)
                    .with_context(|| format!("Failed to load local preset '{}'", preset_name))?;
                return Ok((config, ConfigSource::Preset { 
                    directory: local_dir.clone(), 
                    name: preset_name.to_string() 
                }));
            }
        }
        
        if let Some(ref global_dir) = self.global_dir {
            let preset_file = global_dir.preset_file(preset_name);
            if preset_file.exists() {
                debug!("Loading global preset: {}", preset_file.display());
                let config = self.load_config_file(&preset_file)
                    .with_context(|| format!("Failed to load global preset '{}'", preset_name))?;
                return Ok((config, ConfigSource::Preset { 
                    directory: global_dir.clone(), 
                    name: preset_name.to_string() 
                }));
            }
        }
        
        anyhow::bail!("Preset '{}' not found in local or global directories", preset_name);
    }
    
    /// Load configuration from a specific file
    fn load_config_file(&self, path: &Path) -> Result<ConversationConfig> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        
        let config: ConversationConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        
        debug!("Successfully loaded configuration from: {}", path.display());
        Ok(config)
    }
    
    /// List available presets
    pub fn list_presets(&self) -> Result<Vec<(String, ConfigSource)>> {
        let mut presets = Vec::new();
        
        // Add local presets
        if let Some(ref local_dir) = self.local_dir {
            for preset_name in local_dir.list_presets()? {
                presets.push((preset_name.clone(), ConfigSource::Preset {
                    directory: local_dir.clone(),
                    name: preset_name,
                }));
            }
        }
        
        // Add global presets (if not already present)
        if let Some(ref global_dir) = self.global_dir {
            for preset_name in global_dir.list_presets()? {
                // Only add if not already present from local
                if !presets.iter().any(|(name, _)| name == &preset_name) {
                    presets.push((preset_name.clone(), ConfigSource::Preset {
                        directory: global_dir.clone(),
                        name: preset_name,
                    }));
                }
            }
        }
        
        presets.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(presets)
    }
    
    /// Get the appropriate directory for sessions
    pub fn get_sessions_directory(&self) -> Option<&ThChatDirectory> {
        // Prefer local, fall back to global
        self.local_dir.as_ref().or(self.global_dir.as_ref())
    }
    
    /// Create default configuration files
    pub fn create_default_config(&self, directory: &ThChatDirectory) -> Result<()> {
        info!("Creating default configuration in: {}", directory.root.display());
        
        // Create main config.json
        let default_config = ConversationConfig::default();
        let config_json = serde_json::to_string_pretty(&default_config)?;
        fs::write(&directory.config_file, config_json)
            .with_context(|| format!("Failed to write config file: {}", directory.config_file.display()))?;
        
        // Create example presets
        self.create_example_presets(directory)?;
        
        info!("Created default configuration successfully");
        Ok(())
    }
    
    /// Create example preset files
    fn create_example_presets(&self, directory: &ThChatDirectory) -> Result<()> {
        // Coding preset
        let coding_preset = ConversationConfig {
            model_config: ModelConfig {
                model: "claude-3-5-sonnet-20241022".to_string(),
                provider: "anthropic".to_string(),
            },
            temperature: Some(0.3),
            max_tokens: 8192,
            system_prompt: Some("You are an expert programmer. Provide clean, well-documented code with explanations. Focus on best practices and maintainable solutions.".to_string()),
            title: "Coding Session".to_string(),
            mcp_servers: vec![
                McpServerConfig {
                    actor_id: None,
                    config: McpConfig {
                        command: "simple-fs-mcp-server".to_string(),
                        args: vec!["--allowed-dirs".to_string(), ".".to_string()],
                    },
                    tools: None,
                }
            ],
        };
        
        let coding_json = serde_json::to_string_pretty(&coding_preset)?;
        fs::write(directory.preset_file("coding"), coding_json)?;
        
        // Research preset
        let research_preset = ConversationConfig {
            model_config: ModelConfig {
                model: "gemini-2.5-flash-preview-04-17".to_string(),
                provider: "google".to_string(),
            },
            temperature: Some(0.8),
            max_tokens: 65535,
            system_prompt: Some("You are a research assistant. Provide thorough analysis with multiple perspectives. When possible, suggest sources for further reading.".to_string()),
            title: "Research Session".to_string(),
            mcp_servers: vec![],
        };
        
        let research_json = serde_json::to_string_pretty(&research_preset)?;
        fs::write(directory.preset_file("research"), research_json)?;
        
        debug!("Created example presets: coding, research");
        Ok(())
    }
}

/// Options for loading configuration
#[derive(Debug, Default)]
pub struct ConfigLoadOptions {
    /// Explicit configuration file path
    pub config_file: Option<std::path::PathBuf>,
    
    /// Named preset to load
    pub preset: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;
    
    #[test]
    fn test_default_config() {
        let config = ConversationConfig::default();
        assert_eq!(config.model_config.provider, "google");
        assert_eq!(config.max_tokens, 65535);
        assert_eq!(config.title, "CLI Chat");
    }
    
    #[test]
    fn test_config_serialization() {
        let config = ConversationConfig::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: ConversationConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config.model_config.model, parsed.model_config.model);
    }
    
    #[test]
    fn test_config_manager_fallback() {
        // Create a temporary directory without any .th-chat directories
        let _temp_dir = TempDir::new().unwrap();
        
        let manager = ConfigManager {
            local_dir: None,
            global_dir: None,
        };
        
        let options = ConfigLoadOptions::default();
        let (config, source) = manager.load_config(&options).unwrap();
        
        assert_eq!(config.model_config.provider, "google");
        assert!(matches!(source, ConfigSource::Default));
    }
}

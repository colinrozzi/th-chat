use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::fs;
use tracing::{debug, info, warn};

/// Standard .th-chat directory name
pub const TH_CHAT_DIR: &str = ".th-chat";

/// Configuration file name
pub const CONFIG_FILE: &str = "config.json";

/// Sessions subdirectory name
pub const SESSIONS_DIR: &str = "sessions";

/// Presets subdirectory name
pub const PRESETS_DIR: &str = "presets";

/// MCP configurations subdirectory name
pub const MCP_DIR: &str = "mcp";

/// Represents a .th-chat directory and its structure
#[derive(Debug, Clone)]
pub struct ThChatDirectory {
    /// Root path of the .th-chat directory
    pub root: PathBuf,
    
    /// Path to config.json
    pub config_file: PathBuf,
    
    /// Path to sessions/ subdirectory
    pub sessions_dir: PathBuf,
    
    /// Path to presets/ subdirectory
    pub presets_dir: PathBuf,
    
    /// Path to mcp/ subdirectory
    pub mcp_dir: PathBuf,
}

impl ThChatDirectory {
    /// Create a new ThChatDirectory from a root path
    pub fn new(root: PathBuf) -> Self {
        let config_file = root.join(CONFIG_FILE);
        let sessions_dir = root.join(SESSIONS_DIR);
        let presets_dir = root.join(PRESETS_DIR);
        let mcp_dir = root.join(MCP_DIR);
        
        Self {
            root,
            config_file,
            sessions_dir,
            presets_dir,
            mcp_dir,
        }
    }
    
    /// Check if this directory exists and has the expected structure
    pub fn exists(&self) -> bool {
        self.root.exists() && self.root.is_dir()
    }
    
    /// Check if the config file exists
    pub fn has_config(&self) -> bool {
        self.config_file.exists() && self.config_file.is_file()
    }
    
    /// Create the directory structure
    pub fn create(&self) -> Result<()> {
        info!("Creating .th-chat directory structure at: {}", self.root.display());
        
        // Create main directory
        fs::create_dir_all(&self.root)
            .with_context(|| format!("Failed to create directory: {}", self.root.display()))?;
        
        // Create subdirectories
        fs::create_dir_all(&self.sessions_dir)
            .with_context(|| format!("Failed to create sessions directory: {}", self.sessions_dir.display()))?;
            
        fs::create_dir_all(&self.presets_dir)
            .with_context(|| format!("Failed to create presets directory: {}", self.presets_dir.display()))?;
            
        fs::create_dir_all(&self.mcp_dir)
            .with_context(|| format!("Failed to create mcp directory: {}", self.mcp_dir.display()))?;
        
        debug!("Created .th-chat directory structure successfully");
        Ok(())
    }
    
    /// Get the path to a session file
    pub fn session_file(&self, conversation_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.json", conversation_id))
    }
    
    /// Get the path to a preset file
    pub fn preset_file(&self, preset_name: &str) -> PathBuf {
        self.presets_dir.join(format!("{}.json", preset_name))
    }
    
    /// List available presets
    pub fn list_presets(&self) -> Result<Vec<String>> {
        if !self.presets_dir.exists() {
            return Ok(vec![]);
        }
        
        let mut presets = Vec::new();
        
        for entry in fs::read_dir(&self.presets_dir)
            .with_context(|| format!("Failed to read presets directory: {}", self.presets_dir.display()))? 
        {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    presets.push(name.to_string());
                }
            }
        }
        
        presets.sort();
        Ok(presets)
    }
    
    /// List existing sessions
    pub fn list_sessions(&self) -> Result<Vec<String>> {
        if !self.sessions_dir.exists() {
            return Ok(vec![]);
        }
        
        let mut sessions = Vec::new();
        
        for entry in fs::read_dir(&self.sessions_dir)
            .with_context(|| format!("Failed to read sessions directory: {}", self.sessions_dir.display()))? 
        {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    sessions.push(name.to_string());
                }
            }
        }
        
        sessions.sort();
        Ok(sessions)
    }
}

/// Find .th-chat directory by searching up the directory tree
pub fn find_th_chat_dir() -> Option<ThChatDirectory> {
    let current_dir = std::env::current_dir().ok()?;
    find_th_chat_dir_from(&current_dir)
}

/// Find .th-chat directory starting from a specific path
pub fn find_th_chat_dir_from(start_path: &Path) -> Option<ThChatDirectory> {
    let mut current = start_path;
    
    loop {
        let th_chat_path = current.join(TH_CHAT_DIR);
        debug!("Checking for .th-chat directory at: {}", th_chat_path.display());
        
        if th_chat_path.exists() && th_chat_path.is_dir() {
            debug!("Found .th-chat directory at: {}", th_chat_path.display());
            return Some(ThChatDirectory::new(th_chat_path));
        }
        
        // Move up one directory
        current = current.parent()?;
    }
}

/// Get the global .th-chat directory (in user's home directory)
pub fn get_global_th_chat_dir() -> Option<ThChatDirectory> {
    let home_dir = dirs::home_dir()?;
    let global_th_chat = home_dir.join(TH_CHAT_DIR);
    Some(ThChatDirectory::new(global_th_chat))
}

/// Create a .th-chat directory in the current directory
pub fn create_local_th_chat_dir() -> Result<ThChatDirectory> {
    let current_dir = std::env::current_dir()
        .context("Failed to get current directory")?;
    let th_chat_path = current_dir.join(TH_CHAT_DIR);
    let directory = ThChatDirectory::new(th_chat_path);
    directory.create()?;
    Ok(directory)
}

/// Create a .th-chat directory in the user's home directory
pub fn create_global_th_chat_dir() -> Result<ThChatDirectory> {
    let home_dir = dirs::home_dir()
        .context("Failed to get home directory")?;
    let th_chat_path = home_dir.join(TH_CHAT_DIR);
    let directory = ThChatDirectory::new(th_chat_path);
    directory.create()?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_th_chat_directory_creation() {
        let temp_dir = TempDir::new().unwrap();
        let th_chat_path = temp_dir.path().join(TH_CHAT_DIR);
        let directory = ThChatDirectory::new(th_chat_path);
        
        assert!(!directory.exists());
        directory.create().unwrap();
        assert!(directory.exists());
        assert!(directory.sessions_dir.exists());
        assert!(directory.presets_dir.exists());
        assert!(directory.mcp_dir.exists());
    }
    
    #[test]
    fn test_find_th_chat_dir() {
        let temp_dir = TempDir::new().unwrap();
        let nested_dir = temp_dir.path().join("project").join("subdir");
        fs::create_dir_all(&nested_dir).unwrap();
        
        // Create .th-chat in the temp_dir
        let th_chat_path = temp_dir.path().join(TH_CHAT_DIR);
        fs::create_dir(&th_chat_path).unwrap();
        
        // Search from nested directory should find the .th-chat directory
        let found = find_th_chat_dir_from(&nested_dir);
        assert!(found.is_some());
        assert_eq!(found.unwrap().root, th_chat_path);
    }
}

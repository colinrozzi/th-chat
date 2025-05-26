use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

const SESSION_FILE: &str = ".th-chat";

/// Session data stored locally to persist chat sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub conversation_id: String,
    pub store_id: String,
    pub created_at: u64,
    pub last_accessed: u64,
}

impl SessionData {
    pub fn new(conversation_id: String, store_id: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            conversation_id,
            store_id,
            created_at: now,
            last_accessed: now,
        }
    }

    pub fn update_access_time(&mut self) {
        self.last_accessed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }
}

/// Get the path to the session file (either provided or in current directory)
pub fn get_session_file_path(working_dir: Option<&Path>) -> PathBuf {
    match working_dir {
        Some(dir) => dir.join(SESSION_FILE),
        None => PathBuf::from(SESSION_FILE),
    }
}

/// Check if a session file exists
pub fn session_exists(working_dir: Option<&Path>) -> bool {
    let session_path = get_session_file_path(working_dir);
    session_path.exists()
}

/// Load existing session data from file
pub fn load_session(working_dir: Option<&Path>) -> Result<SessionData> {
    let session_path = get_session_file_path(working_dir);
    
    info!("Loading session from: {}", session_path.display());
    
    let content = fs::read_to_string(&session_path)
        .with_context(|| format!("Failed to read session file: {}", session_path.display()))?;
    
    let mut session_data: SessionData = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse session file: {}", session_path.display()))?;
    
    // Update access time
    session_data.update_access_time();
    
    info!(
        "Loaded session - conversation_id: {}, store_id: {}, created: {}",
        session_data.conversation_id,
        session_data.store_id,
        session_data.created_at
    );
    
    Ok(session_data)
}

/// Save session data to file
pub fn save_session(session_data: &SessionData, working_dir: Option<&Path>) -> Result<()> {
    let session_path = get_session_file_path(working_dir);
    
    info!("Saving session to: {}", session_path.display());
    debug!("Session data: {:?}", session_data);
    
    let content = serde_json::to_string_pretty(session_data)
        .context("Failed to serialize session data")?;
    
    fs::write(&session_path, content)
        .with_context(|| format!("Failed to write session file: {}", session_path.display()))?;
    
    info!("Session saved successfully");
    Ok(())
}

/// Remove/delete the session file
pub fn clear_session(working_dir: Option<&Path>) -> Result<()> {
    let session_path = get_session_file_path(working_dir);
    
    if session_path.exists() {
        info!("Removing session file: {}", session_path.display());
        fs::remove_file(&session_path)
            .with_context(|| format!("Failed to remove session file: {}", session_path.display()))?;
        info!("Session file removed successfully");
    } else {
        warn!("Session file does not exist: {}", session_path.display());
    }
    
    Ok(())
}

/// Update the last accessed time for an existing session
pub fn update_session_access_time(working_dir: Option<&Path>) -> Result<()> {
    if !session_exists(working_dir) {
        return Ok(()); // No session to update
    }
    
    let mut session_data = load_session(working_dir)?;
    session_data.update_access_time();
    save_session(&session_data, working_dir)?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_session_persistence() {
        let temp_dir = tempdir().unwrap();
        let temp_path = temp_dir.path();
        
        // Test no session exists initially
        assert!(!session_exists(Some(temp_path)));
        
        // Create and save a session
        let session = SessionData::new(
            "conv-123".to_string(),
            "store-456".to_string(),
        );
        save_session(&session, Some(temp_path)).unwrap();
        
        // Test session exists now
        assert!(session_exists(Some(temp_path)));
        
        // Load and verify session
        let loaded_session = load_session(Some(temp_path)).unwrap();
        assert_eq!(loaded_session.conversation_id, "conv-123");
        assert_eq!(loaded_session.store_id, "store-456");
        assert!(loaded_session.last_accessed >= loaded_session.created_at);
        
        // Clear session
        clear_session(Some(temp_path)).unwrap();
        assert!(!session_exists(Some(temp_path)));
    }
}

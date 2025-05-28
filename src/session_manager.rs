use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Enhanced session data with metadata and naming support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub name: String,
    pub conversation_id: String,
    pub store_id: String,
    pub created_at: u64,
    pub last_accessed: u64,
    pub description: Option<String>,
    pub config_preset: Option<String>,
    pub message_count: u32,
}

impl SessionData {
    pub fn new(name: String, conversation_id: String, store_id: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        
        Self {
            name,
            conversation_id,
            store_id,
            created_at: now,
            last_accessed: now,
            description: None,
            config_preset: None,
            message_count: 0,
        }
    }

    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    pub fn with_preset(mut self, preset: String) -> Self {
        self.config_preset = Some(preset);
        self
    }

    pub fn update_access_time(&mut self) {
        self.last_accessed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
    }

    pub fn increment_message_count(&mut self) {
        self.message_count += 1;
        self.update_access_time();
    }
}

/// Session information for listing and display
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub description: Option<String>,
    pub created_at: u64,
    pub last_accessed: u64,
    pub message_count: u32,
    pub config_preset: Option<String>,
    pub file_path: PathBuf,
}

impl SessionInfo {
    pub fn from_session_data(session: &SessionData, file_path: PathBuf) -> Self {
        Self {
            name: session.name.clone(),
            description: session.description.clone(),
            created_at: session.created_at,
            last_accessed: session.last_accessed,
            message_count: session.message_count,
            config_preset: session.config_preset.clone(),
            file_path,
        }
    }

    pub fn age_hours(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        (now - self.last_accessed) / 3600
    }

    pub fn is_older_than_days(&self, days: u64) -> bool {
        self.age_hours() > (days * 24)
    }
}

/// Manager for handling multiple chat sessions
pub struct SessionManager {
    sessions_dir: PathBuf,
    current_session_name: Option<String>,
}

impl SessionManager {
    /// Create a new SessionManager for the given sessions directory
    pub fn new(sessions_dir: PathBuf) -> Result<Self> {
        // Ensure sessions directory exists
        if !sessions_dir.exists() {
            fs::create_dir_all(&sessions_dir)
                .with_context(|| format!("Failed to create sessions directory: {}", sessions_dir.display()))?;
            info!("Created sessions directory: {}", sessions_dir.display());
        }

        let mut manager = Self {
            sessions_dir,
            current_session_name: None,
        };

        // Check for legacy session and migrate if needed
        manager.migrate_legacy_session()?;

        Ok(manager)
    }

    /// Get the default session name
    pub fn default_session_name() -> &'static str {
        "default"
    }

    /// Get path to a session file
    pub fn session_file_path(&self, name: &str) -> PathBuf {
        self.sessions_dir.join(format!("{}.json", name))
    }

    /// Check if a session exists
    pub fn session_exists(&self, name: &str) -> bool {
        self.session_file_path(name).exists()
    }

    /// List all available sessions
    pub fn list_sessions(&self) -> Result<Vec<SessionInfo>> {
        let mut sessions = Vec::new();

        if !self.sessions_dir.exists() {
            return Ok(sessions);
        }

        let entries = fs::read_dir(&self.sessions_dir)
            .with_context(|| format!("Failed to read sessions directory: {}", self.sessions_dir.display()))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            
            if path.is_file() && path.extension().map_or(false, |ext| ext == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    match self.load_session(stem) {
                        Ok(session_data) => {
                            sessions.push(SessionInfo::from_session_data(&session_data, path));
                        },
                        Err(e) => {
                            warn!("Failed to load session '{}': {}", stem, e);
                        }
                    }
                }
            }
        }

        // Sort by last accessed time (most recent first)
        sessions.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));

        Ok(sessions)
    }

    /// Load a session by name
    pub fn load_session(&self, name: &str) -> Result<SessionData> {
        let session_path = self.session_file_path(name);
        
        debug!("Loading session '{}' from: {}", name, session_path.display());
        
        let content = fs::read_to_string(&session_path)
            .with_context(|| format!("Failed to read session file: {}", session_path.display()))?;
        
        let mut session_data: SessionData = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse session file: {}", session_path.display()))?;
        
        // Update access time
        session_data.update_access_time();
        
        info!(
            "Loaded session '{}' - conversation_id: {}, store_id: {}, messages: {}",
            session_data.name,
            session_data.conversation_id,
            session_data.store_id,
            session_data.message_count
        );
        
        Ok(session_data)
    }

    /// Save a session
    pub fn save_session(&self, session_data: &SessionData) -> Result<()> {
        let session_path = self.session_file_path(&session_data.name);
        
        debug!("Saving session '{}' to: {}", session_data.name, session_path.display());
        
        let content = serde_json::to_string_pretty(session_data)
            .context("Failed to serialize session data")?;
        
        fs::write(&session_path, content)
            .with_context(|| format!("Failed to write session file: {}", session_path.display()))?;
        
        info!("Session '{}' saved successfully", session_data.name);
        Ok(())
    }

    /// Create a new session
    pub fn create_session(
        &self,
        name: &str,
        conversation_id: String,
        store_id: String,
        description: Option<String>,
        preset: Option<String>,
    ) -> Result<SessionData> {
        if self.session_exists(name) {
            return Err(anyhow::anyhow!("Session '{}' already exists", name));
        }

        let mut session = SessionData::new(name.to_string(), conversation_id, store_id);
        
        if let Some(desc) = description {
            session = session.with_description(desc);
        }
        
        if let Some(preset_name) = preset {
            session = session.with_preset(preset_name);
        }

        self.save_session(&session)?;
        info!("Created new session '{}'", name);
        
        Ok(session)
    }

    /// Delete a session
    pub fn delete_session(&self, name: &str) -> Result<()> {
        if name == Self::default_session_name() {
            return Err(anyhow::anyhow!("Cannot delete the default session"));
        }

        let session_path = self.session_file_path(name);
        
        if !session_path.exists() {
            return Err(anyhow::anyhow!("Session '{}' does not exist", name));
        }

        fs::remove_file(&session_path)
            .with_context(|| format!("Failed to delete session file: {}", session_path.display()))?;
        
        info!("Deleted session '{}'", name);
        Ok(())
    }

    /// Rename a session
    pub fn rename_session(&self, old_name: &str, new_name: &str) -> Result<()> {
        if old_name == Self::default_session_name() {
            return Err(anyhow::anyhow!("Cannot rename the default session"));
        }

        if self.session_exists(new_name) {
            return Err(anyhow::anyhow!("Session '{}' already exists", new_name));
        }

        let mut session_data = self.load_session(old_name)?;
        session_data.name = new_name.to_string();
        
        self.save_session(&session_data)?;
        self.delete_session(old_name)?;
        
        info!("Renamed session '{}' to '{}'", old_name, new_name);
        Ok(())
    }

    /// Clean old sessions based on age or inactivity
    pub fn clean_sessions(&self, older_than_days: Option<u64>, dry_run: bool) -> Result<Vec<String>> {
        let sessions = self.list_sessions()?;
        let mut deleted_sessions = Vec::new();

        for session in sessions {
            let should_delete = if let Some(days) = older_than_days {
                session.is_older_than_days(days)
            } else {
                false
            };

            if should_delete && session.name != Self::default_session_name() {
                if dry_run {
                    info!("Would delete session '{}' (inactive for {} hours)", 
                         session.name, session.age_hours());
                } else {
                    match self.delete_session(&session.name) {
                        Ok(_) => {
                            deleted_sessions.push(session.name.clone());
                        },
                        Err(e) => {
                            warn!("Failed to delete session '{}': {}", session.name, e);
                        }
                    }
                }
            }
        }

        if !dry_run && !deleted_sessions.is_empty() {
            info!("Cleaned {} old sessions", deleted_sessions.len());
        }

        Ok(deleted_sessions)
    }

    /// Migrate legacy .th-chat session file to default.json
    fn migrate_legacy_session(&self) -> Result<()> {
        let legacy_path = self.sessions_dir.join(".th-chat");
        
        if !legacy_path.exists() {
            return Ok(());
        }

        info!("Found legacy session file, migrating to default.json");

        // Read legacy session
        let content = fs::read_to_string(&legacy_path)
            .context("Failed to read legacy session file")?;

        // Parse as old SessionData format (without name field)
        #[derive(Deserialize)]
        struct LegacySessionData {
            conversation_id: String,
            store_id: String,
            created_at: u64,
            last_accessed: u64,
        }

        let legacy_session: LegacySessionData = serde_json::from_str(&content)
            .context("Failed to parse legacy session file")?;

        // Create new session with default name
        let new_session = SessionData {
            name: Self::default_session_name().to_string(),
            conversation_id: legacy_session.conversation_id,
            store_id: legacy_session.store_id,
            created_at: legacy_session.created_at,
            last_accessed: legacy_session.last_accessed,
            description: Some("Migrated from legacy session".to_string()),
            config_preset: None,
            message_count: 0, // We don't know the count from legacy
        };

        // Save as new format
        self.save_session(&new_session)?;

        // Remove legacy file
        fs::remove_file(&legacy_path)
            .context("Failed to remove legacy session file")?;

        info!("Successfully migrated legacy session to 'default'");
        Ok(())
    }

    /// Get the session to use based on preferences
    pub fn resolve_session_name(&self, requested_name: Option<&str>) -> String {
        if let Some(name) = requested_name {
            if self.session_exists(name) {
                return name.to_string();
            } else {
                warn!("Requested session '{}' does not exist, using default", name);
            }
        }

        Self::default_session_name().to_string()
    }

    /// Update session metadata (message count, access time)
    pub fn update_session_metadata(&self, name: &str) -> Result<()> {
        let mut session_data = self.load_session(name)?;
        session_data.increment_message_count();
        self.save_session(&session_data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_session_manager_creation() {
        let temp_dir = tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("sessions");
        
        let manager = SessionManager::new(sessions_dir.clone()).unwrap();
        assert!(sessions_dir.exists());
    }

    #[test]
    fn test_session_crud_operations() {
        let temp_dir = tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("sessions");
        let manager = SessionManager::new(sessions_dir).unwrap();

        // Create session
        let session = manager.create_session(
            "test",
            "conv-123".to_string(),
            "store-456".to_string(),
            Some("Test session".to_string()),
            None,
        ).unwrap();

        assert_eq!(session.name, "test");
        assert!(manager.session_exists("test"));

        // Load session
        let loaded = manager.load_session("test").unwrap();
        assert_eq!(loaded.conversation_id, "conv-123");
        assert_eq!(loaded.description, Some("Test session".to_string()));

        // List sessions
        let sessions = manager.list_sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "test");

        // Delete session
        manager.delete_session("test").unwrap();
        assert!(!manager.session_exists("test"));
    }

    #[test]
    fn test_legacy_migration() {
        let temp_dir = tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();

        // Create legacy session file
        let legacy_content = r#"{
            "conversation_id": "legacy-conv",
            "store_id": "legacy-store",
            "created_at": 1234567890,
            "last_accessed": 1234567890
        }"#;
        
        fs::write(sessions_dir.join(".th-chat"), legacy_content).unwrap();

        // Create manager (should trigger migration)
        let manager = SessionManager::new(sessions_dir.clone()).unwrap();

        // Check migration worked
        assert!(!sessions_dir.join(".th-chat").exists());
        assert!(manager.session_exists("default"));

        let default_session = manager.load_session("default").unwrap();
        assert_eq!(default_session.conversation_id, "legacy-conv");
        assert_eq!(default_session.store_id, "legacy-store");
    }
}

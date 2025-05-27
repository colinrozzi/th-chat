# Change Request: .th-chat Directory Configuration System

**Created:** 2025-05-27  
**Status:** Proposed  
**Type:** Feature Enhancement  

## Overview

Replace the current individual CLI flag approach with a unified `.th-chat` directory-based configuration system that provides better organization, project-scoped settings, and preset management.

## Current State

### Problems with Current Approach
- CLI flags for individual settings (`--model`, `--provider`, `--temperature`, etc.)
- Inconsistent mapping between CLI args and `ConversationSettings` struct
- No project-scoped configuration
- Session files scattered in current directory or specified paths
- MCP configuration as separate JSON file
- Limited reusability and sharing of configurations

### Current CLI Interface
```bash
th-chat --server 127.0.0.1:9000 \
        --model gemini-2.5-flash-preview-04-17 \
        --provider google \
        --temperature 0.7 \
        --max-tokens 65535 \
        --system-prompt "You are a helpful assistant" \
        --mcp-config ./mcp-config.json
```

## Proposed Solution

### .th-chat Directory Structure
```
.th-chat/
├── config.json              # Default conversation settings (ConversationSettings format)
├── sessions/                 # Session persistence files
│   ├── {conversation-id}.json
│   └── {conversation-id}.json
├── presets/                  # Named configuration presets
│   ├── coding.json
│   ├── writing.json
│   ├── research.json
│   └── default.json
└── mcp/                      # MCP server configurations (optional organization)
    ├── filesystem.json
    ├── web-search.json
    └── development.json
```

### Global Configuration Support
```
~/.th-chat/                   # User-wide defaults
├── config.json              # Global default settings
└── presets/                 # User-wide presets
    ├── personal.json
    └── work.json
```

## Configuration Schema

### Base Configuration Format
All configuration files follow the `ConversationSettings` structure from the chat-state actor:

```json
{
  "model_config": {
    "model": "gemini-2.5-flash-preview-04-17",
    "provider": "google"
  },
  "temperature": 0.7,
  "max_tokens": 65535,
  "system_prompt": "You are a helpful assistant specialized in...",
  "title": "Development Chat",
  "mcp_servers": [
    {
      "config": {
        "command": "/path/to/simple-fs-mcp-server",
        "args": ["--allowed-dirs", ".", "/tmp"]
      },
      "actor_id": null,
      "tools": null
    }
  ]
}
```

### Preset Examples

**coding.json:**
```json
{
  "model_config": {
    "model": "claude-3-5-sonnet-20241022",
    "provider": "anthropic"
  },
  "temperature": 0.3,
  "max_tokens": 8192,
  "system_prompt": "You are an expert programmer. Provide clean, well-documented code with explanations.",
  "title": "Coding Session",
  "mcp_servers": [
    {
      "config": {
        "command": "simple-fs-mcp-server",
        "args": ["--allowed-dirs", ".", "src/", "tests/"]
      }
    }
  ]
}
```

**research.json:**
```json
{
  "model_config": {
    "model": "gemini-2.5-flash-preview-04-17",
    "provider": "google"
  },
  "temperature": 0.8,
  "max_tokens": 65535,
  "system_prompt": "You are a research assistant. Provide thorough analysis with citations and multiple perspectives.",
  "title": "Research Session",
  "mcp_servers": []
}
```

## New CLI Interface

### Simplified Commands
```bash
# Use default configuration
th-chat

# Use specific preset
th-chat --preset coding

# Use custom config file
th-chat --config ./custom-config.json

# Management commands
th-chat init                    # Initialize .th-chat directory
th-chat presets                 # List available presets
th-chat sessions               # List current sessions
th-chat sessions clean         # Clean old sessions
```

### Configuration Resolution Order
1. CLI-specified config file (`--config`)
2. CLI-specified preset (`--preset`)
3. Project config (`./.th-chat/config.json`)
4. Global config (`~/.th-chat/config.json`)
5. Built-in defaults

## Implementation Plan

### Phase 1: Core Infrastructure
1. **Directory Management**
   - Add functions to find and create `.th-chat` directories
   - Implement configuration resolution hierarchy
   - Add JSON schema validation for configuration files

2. **Configuration Loading**
   - Replace individual CLI args with unified config loading
   - Update `Args` struct to support new approach
   - Maintain `ConversationSettings` format for actor compatibility

3. **Session Management**
   - Move session persistence to `.th-chat/sessions/`
   - Update session file naming and organization
   - Add session cleanup utilities

### Phase 2: Preset System
1. **Preset Management**
   - Implement preset loading from `presets/` directories
   - Add preset validation and error handling
   - Support both local and global presets

2. **CLI Enhancements**
   - Add `--preset` flag
   - Implement preset listing command
   - Add helpful error messages for missing presets

### Phase 3: Management Commands
1. **Initialization**
   - `th-chat init` command to set up directory structure
   - Generate example presets and default config
   - Interactive configuration wizard option

2. **Utility Commands**
   - `th-chat presets` - list available presets
   - `th-chat sessions` - session management
   - `th-chat config` - show resolved configuration

## Code Changes Required

### New Files
- `src/config_manager.rs` - Configuration discovery and loading
- `src/directory.rs` - `.th-chat` directory management
- `src/presets.rs` - Preset handling
- `src/commands/` - Management command implementations

### Modified Files
- `src/config.rs` - Update `Args` struct, remove individual setting fields
- `src/main.rs` - Update initialization to use new config system  
- `src/persistence.rs` - Update session paths to use `.th-chat/sessions/`
- `src/chat.rs` - Update to work with unified configuration loading

### Removed Functionality
- Individual CLI flags for model settings (breaking change)
- Current session persistence logic (replaced with directory-based)

## Benefits

### Developer Experience
- **Project-scoped configurations** - Different settings per project
- **Preset system** - Easy switching between common configurations
- **Organized file management** - All th-chat files in one place
- **Team collaboration** - Shareable configuration presets

### Maintainability
- **Unified configuration format** - Single source of truth
- **Reduced CLI complexity** - Fewer flags to maintain
- **Better extensibility** - Easy to add new settings without CLI changes
- **Consistent with actor format** - Direct `ConversationSettings` usage

### User Workflow
- **Cleaner command line** - Simple commands for common tasks
- **Better organization** - `.gitignore` friendly structure
- **Discovery** - Easy to see and modify configurations
- **Flexibility** - Multiple ways to specify configuration

## Risks and Considerations

### Breaking Changes
- **CLI interface change** - Current flag-based usage will break
- **Session file locations** - Existing sessions need migration
- **Migration path needed** - For existing users and scripts

### Implementation Complexity
- **Configuration resolution** - Complex hierarchy and validation logic
- **Error handling** - Good error messages for configuration issues
- **Cross-platform paths** - Handle different OS path conventions

### User Adoption
- **Learning curve** - Users need to understand new directory structure
- **Documentation** - Comprehensive docs and examples needed
- **Migration tooling** - Consider helper commands for migration

## Success Criteria

1. **Functional Requirements**
   - ✅ Load configuration from `.th-chat/config.json`
   - ✅ Support preset system with `--preset` flag
   - ✅ Maintain full compatibility with `ConversationSettings`
   - ✅ Session persistence in `.th-chat/sessions/`
   - ✅ Configuration hierarchy (global → local → preset)

2. **User Experience**
   - ✅ Simple CLI for common use cases
   - ✅ Clear error messages for configuration issues
   - ✅ Easy project initialization with `th-chat init`
   - ✅ Good documentation and examples

3. **Technical Quality**
   - ✅ Clean separation of configuration logic
   - ✅ Robust error handling and validation
   - ✅ Maintainable and extensible code structure

## Future Enhancements

- **Configuration templates** - Common project type templates
- **Environment variable substitution** - Dynamic configuration values
- **Configuration validation** - JSON schema and runtime validation
- **Web interface** - GUI for configuration management
- **Preset marketplace** - Community-shared preset repository
- **Conversation templates** - Pre-built conversation starters
- **Configuration inheritance** - More sophisticated merging strategies

---

**Next Steps:**
1. Review and approve this change request
2. Create detailed technical specification
3. Begin implementation with Phase 1
4. Create migration documentation and tooling

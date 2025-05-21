# th-chat

A simple CLI tool to interact with the chat-state actor in the Theater runtime.

## Overview

`th-chat` is a streamlined chat interface that:

1. Connects to a Theater server
2. Starts a chat-state actor to manage a conversation
3. Provides a REPL (Read-Eval-Print Loop) interface for chatting
4. Displays responses from the AI model

## Installation

```bash
# Clone the repository
git clone <repository-url>

# Build the tool
cd th-chat
cargo build --release

# Create a symbolic link (optional)
ln -s $(pwd)/target/release/th-chat /usr/local/bin/th-chat
```

## Usage

Simply run the command:

```bash
th-chat
```

### Command Line Options

- `--server`: Address of the Theater server (default: 127.0.0.1:9000)
- `--model`: AI model to use (default: claude-3-5-sonnet-20240307)
- `--provider`: Provider to use (default: anthropic)
- `--system-prompt`: Custom system prompt to use

### Special Commands

Once in the chat interface, you can use these special commands:

- `/exit`: Exit the program
- `/clear`: Clear the screen
- `/help`: Show available commands

### Environment Variables

- `THEATER_SERVER_ADDRESS`: Address of the Theater server
- `THEATER_CHAT_MODEL`: AI model to use
- `THEATER_CHAT_PROVIDER`: Provider to use
- `THEATER_CHAT_SYSTEM_PROMPT`: Custom system prompt

## Requirements

- Rust 1.70+
- A running Theater server
- Access to the chat-state actor

## How It Works

Behind the scenes, `th-chat`:

1. Connects to a Theater server
2. Starts the chat-state actor with a unique conversation ID
3. Configures the actor with the specified model settings
4. Enters a REPL loop for user interaction
5. Sends user messages to the actor and displays responses

## License

[MIT License](LICENSE)

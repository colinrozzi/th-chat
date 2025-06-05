# OpenCode Configuration for th-chat

## Build/Test/Lint Commands
- **Build**: `cargo build` (debug) or `cargo build --release` (optimized)
- **Run**: `cargo run` or `cargo run --release`
- **Test**: `cargo test` (runs all tests)
- **Test single**: `cargo test <test_name>` or `cargo test --test <test_file>`
- **Check**: `cargo check` (fast compile check without building)
- **Clippy**: `cargo clippy` (linting)
- **Format**: `cargo fmt` (code formatting)

## Code Style Guidelines
- **Imports**: Use `use` statements, group std/external/local, alphabetical within groups
- **Error handling**: Use `anyhow::Result<T>` for functions that can fail, `?` operator for propagation
- **Naming**: snake_case for functions/variables, PascalCase for types/structs, SCREAMING_SNAKE_CASE for constants
- **Types**: Prefer explicit types for public APIs, use `impl Trait` for complex return types
- **Async**: Use `async/await` with `tokio`, prefer `async fn` over `impl Future`
- **Logging**: Use `tracing` crate with appropriate levels (debug, info, warn, error)
- **Serialization**: Use `serde` with `#[derive(Serialize, Deserialize)]` for data structures
- **CLI**: Use `clap` with derive API for command-line parsing
- **Comments**: Minimal inline comments, prefer self-documenting code and doc comments for public APIs
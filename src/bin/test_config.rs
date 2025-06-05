use anyhow::Result;
use std::path::PathBuf;

// Import the modules we created
use th_chat::config_manager::{ConfigLoadOptions, ConfigManager};
use th_chat::directory::create_local_th_chat_dir;

#[tokio::main]
async fn main() -> Result<()> {
    // Test 1: Test configuration loading from file
    println!("🧪 Testing configuration loading...");

    let config_manager = ConfigManager::new();
    let config_options = ConfigLoadOptions {
        config_file: Some(PathBuf::from("test-dir/test-config.json")),
        preset: None,
    };

    match config_manager.load_config(&config_options) {
        Ok((config, source)) => {
            println!("✅ Successfully loaded configuration from: {}", source);
            println!(
                "   Model: {} ({})",
                config.model_config.model, config.model_config.provider
            );
            println!("   Title: {}", config.title);
            println!("   Max tokens: {}", config.max_tokens);
            if let Some(temp) = config.temperature {
                println!("   Temperature: {}", temp);
            }
        }
        Err(e) => {
            println!("❌ Failed to load configuration: {}", e);
            return Err(e);
        }
    }

    // Test 3: Test directory creation
    println!("\n🧪 Testing directory creation...");
    let test_dir = PathBuf::from("test-dir");
    std::env::set_current_dir(&test_dir)?;

    match create_local_th_chat_dir() {
        Ok(directory) => {
            println!(
                "✅ Successfully created .th-chat directory at: {}",
                directory.root.display()
            );

            // Test preset listing
            let presets = directory.list_presets()?;
            println!("   Found {} presets", presets.len());

            // Test config creation
            match config_manager.create_default_config(&directory) {
                Ok(()) => println!("✅ Successfully created default configuration"),
                Err(e) => println!("❌ Failed to create default configuration: {}", e),
            }

            // Test preset listing after creation
            let presets_after = directory.list_presets()?;
            println!("   Found {} presets after creation", presets_after.len());
            for preset in presets_after {
                println!("     - {}", preset);
            }
        }
        Err(e) => {
            println!("❌ Failed to create directory: {}", e);
        }
    }

    println!("\n🎉 All tests completed!");
    Ok(())
}

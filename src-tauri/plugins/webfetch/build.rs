// No JS-facing commands: the plugin is driven from Rust only
// (`scraper_engine` on Android), so it needs no capability permissions.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).android_path("android").build();
}

fn main() {
    // Network behavior starts in Phase 3. Keeping the host buildable now
    // prevents the shared crates from drifting back toward Tauri coupling.
    println!(
        "notes-server scaffold (sync format v{})",
        notes_sync::FORMAT_VERSION
    );
}

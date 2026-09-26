use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo::rerun-if-env-changed=CLUMSIES_AGENT_RUNTIME_BUILD_ID");
    println!("cargo::rerun-if-changed=src");
    let build_id = std::env::var("CLUMSIES_AGENT_RUNTIME_BUILD_ID").unwrap_or_else(|_| {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        format!("cargo-{timestamp}-{}", std::process::id())
    });
    assert!(
        !build_id.is_empty() && build_id.len() <= 128 && build_id.is_ascii(),
        "CLUMSIES_AGENT_RUNTIME_BUILD_ID must be 1-128 ASCII bytes"
    );
    println!("cargo::rustc-env=CLUMSIES_AGENT_RUNTIME_BUILD_ID={build_id}");
}

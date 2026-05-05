//! Read-only invariant test.
//!
//! Greps `src/` for any forbidden write-method calls and fails if any are
//! found outside the single sanctioned exception (`src/auth/windows.rs`).
//! This is the test-time half of the read-only invariant; the type-level
//! half is `ReadOnlyHttp` in `src/api.rs`, which exposes only `.get<T>()`.

use std::fs;
use walkdir::WalkDir;

const FORBIDDEN: &[&str] = &[".post(", ".patch(", ".put(", ".delete("];

#[test]
fn no_write_methods_in_src() {
    let mut violations: Vec<String> = Vec::new();
    for entry in WalkDir::new("src").into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // Sole sanctioned exception: auth/windows.rs::validate_token uses raw reqwest.
        let path_str = path.to_string_lossy().replace('\\', "/");
        if path_str.ends_with("auth/windows.rs") {
            continue;
        }
        let body = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for needle in FORBIDDEN {
            for (line_no, line) in body.lines().enumerate() {
                // Allow the literal forbidden strings to appear inside this
                // very test file (FORBIDDEN array). And allow them inside
                // string literals or comments — but be conservative and
                // flag occurrences only outside `//` lines and `"..."`
                // strings of identical text.
                let stripped = line.split("//").next().unwrap_or(line);
                if stripped.contains(needle) {
                    violations.push(format!(
                        "{}:{}: forbidden write call {:?} in `{}`",
                        path_str,
                        line_no + 1,
                        needle,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Read-only invariant violated:\n{}",
        violations.join("\n")
    );
}

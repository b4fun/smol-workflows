use std::path::{Component, Path};

pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .bytes()
        .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/' | b':' | b'+' | b'=' | b','))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn quote_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn resolve_remote_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        normalize_path(path)
    } else {
        normalize_path(&format!("{}/{}", cwd.trim_end_matches('/'), path))
    }
}

fn normalize_path(path: &str) -> String {
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::RootDir => parts.clear(),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            _ => {}
        }
    }
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_shell_words() {
        assert_eq!(shell_quote("simple/path"), "simple/path");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("can't"), "'can'\\''t'");
        assert_eq!(
            quote_argv(&["echo".into(), "hello world".into()]),
            "echo 'hello world'"
        );
    }

    #[test]
    fn resolves_relative_paths_against_cwd() {
        assert_eq!(resolve_remote_path("/workspace", "a/b"), "/workspace/a/b");
        assert_eq!(
            resolve_remote_path("/workspace", "../tmp/file"),
            "/tmp/file"
        );
        assert_eq!(resolve_remote_path("/workspace", "/var/tmp"), "/var/tmp");
    }
}

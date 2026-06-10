//! Bookmark/tag helpers + protected-bookmark matching (design.md §7,
//! crate-structure.md §3.2: `bookmark.rs`).
//!
//! Bookmarks are movable named pointers in the [`View`](crate::model::View);
//! tags are immutable name→commit pins. Protected bookmarks (exact names + glob
//! suffixes like `release/*`) are where compatibility is enforced by the core —
//! the JJ layer only provides the matcher.

/// Whether `name` matches any pattern in `patterns`. Supports a trailing `*`
/// glob (e.g. `release/*` matches `release/1.0`) and exact names. A bare `*`
/// matches everything.
pub fn is_protected(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| matches_pattern(name, p))
}

fn matches_pattern(name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    name == pattern
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_name_matches() {
        // Arrange
        let patterns = vec!["main".to_string()];

        // Act
        let protected = is_protected("main", &patterns);

        // Assert
        assert!(protected);
        assert!(!is_protected("feature/x", &patterns));
    }

    #[test]
    fn glob_suffix_matches_prefix() {
        // Arrange
        let patterns = vec!["release/*".to_string()];

        // Act / Assert
        assert!(is_protected("release/1.0", &patterns));
        assert!(is_protected("release/", &patterns));
        assert!(!is_protected("main", &patterns));
    }

    #[test]
    fn star_matches_everything() {
        // Arrange
        let patterns = vec!["*".to_string()];

        // Act / Assert
        assert!(is_protected("anything", &patterns));
    }
}

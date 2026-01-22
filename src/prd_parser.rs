//! PRD parser module for extracting tasks from markdown PRD files.
//!
//! This module parses markdown PRD files and extracts user stories with their
//! acceptance criteria to generate initial task lists.

use crate::task::{Task, TaskStatus};
use anyhow::Result;
use std::path::Path;

/// Extracted user story from a PRD.
#[derive(Debug, Clone)]
pub struct UserStory {
    /// User story identifier (e.g., "US-1").
    pub id: String,
    /// User story title.
    pub title: String,
    /// Full user story text (the "As a... I want... so that..." part).
    pub story: Option<String>,
    /// Acceptance criteria.
    pub acceptance: Vec<String>,
}

/// Parse a PRD markdown file and extract user stories.
///
/// Looks for sections in formats like:
/// - `### US-1: Title`
/// - `### User Story 1: Title`
/// - `## US-1 Title`
///
/// And extracts acceptance criteria from bullet points under "Acceptance:" subsections.
pub fn parse_prd(content: &str) -> Vec<UserStory> {
    let mut stories = Vec::new();
    let mut current_story: Option<UserStory> = None;
    let mut in_acceptance = false;
    let mut story_text = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Check for user story header
        if let Some(story) = parse_user_story_header(trimmed) {
            // Save previous story if any
            if let Some(mut prev) = current_story.take() {
                if !story_text.trim().is_empty() {
                    prev.story = Some(story_text.trim().to_string());
                }
                stories.push(prev);
            }
            current_story = Some(story);
            in_acceptance = false;
            story_text.clear();
            continue;
        }

        // If we're in a user story, look for content
        if current_story.is_some() {
            // Check for acceptance criteria section
            if trimmed.to_lowercase().starts_with("acceptance")
                && (trimmed.ends_with(':') || trimmed.ends_with(":"))
            {
                in_acceptance = true;
                continue;
            }

            // Check for other section headers (end of acceptance)
            if trimmed.starts_with('#') || trimmed.starts_with("**") && trimmed.ends_with("**") {
                // Check if this starts a new section that isn't acceptance
                if !trimmed.to_lowercase().contains("acceptance") {
                    in_acceptance = false;
                }
            }

            // Parse acceptance criteria bullets
            if in_acceptance
                && let Some(criterion) = parse_bullet(trimmed)
                && let Some(ref mut story) = current_story
            {
                story.acceptance.push(criterion);
            } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                // Collect non-header text as story description
                if !story_text.is_empty() {
                    story_text.push(' ');
                }
                story_text.push_str(trimmed);
            }
        }
    }

    // Don't forget the last story
    if let Some(mut story) = current_story {
        if !story_text.trim().is_empty() {
            story.story = Some(story_text.trim().to_string());
        }
        stories.push(story);
    }

    stories
}

/// Parse a user story header line.
/// Returns Some(UserStory) if the line is a valid user story header.
fn parse_user_story_header(line: &str) -> Option<UserStory> {
    // Remove markdown header prefixes
    let stripped = line.trim_start_matches('#').trim();

    // Try patterns like "US-1: Title" or "US-1 Title"
    if stripped.starts_with("US-") || stripped.starts_with("us-") {
        // Find the end of the ID (look for colon or space after "US-N" pattern)
        let id_end = stripped
            .char_indices()
            .find(|(i, c)| *c == ':' || (*c == ' ' && *i > 3)) // After "US-N"
            .map(|(i, _)| i)
            .unwrap_or(stripped.len());

        let id_part = &stripped[..id_end];

        // Handle "US-1:" vs "US-1 "
        let (id, title_start) = if stripped[id_end..].starts_with(':') {
            (id_part.to_string(), id_end + 1)
        } else if stripped[id_end..].starts_with(' ') {
            // Check if there's a colon after spaces
            let after_id = &stripped[id_end..];
            if let Some(colon_pos) = after_id.find(':') {
                // "US-1 : Title" or "US-1: Title"
                (id_part.to_string(), id_end + colon_pos + 1)
            } else {
                (id_part.to_string(), id_end)
            }
        } else {
            (id_part.to_string(), id_end)
        };

        let title = stripped[title_start..].trim().trim_matches('"').to_string();

        return Some(UserStory {
            id: id.to_uppercase(),
            title,
            story: None,
            acceptance: Vec::new(),
        });
    }

    // Try pattern like "User Story N: Title"
    if stripped.to_lowercase().starts_with("user story") {
        let rest = stripped[10..].trim(); // Skip "user story"
        if let Some(colon_pos) = rest.find(':') {
            let id_part = rest[..colon_pos].trim();
            let title = rest[colon_pos + 1..].trim().to_string();

            // Convert "1" or "N" to "US-1" or "US-N"
            let id = if id_part.chars().all(|c| c.is_ascii_digit()) {
                format!("US-{}", id_part)
            } else {
                id_part.to_uppercase()
            };

            return Some(UserStory {
                id,
                title,
                story: None,
                acceptance: Vec::new(),
            });
        }
    }

    None
}

/// Parse a bullet point line and return the content.
fn parse_bullet(line: &str) -> Option<String> {
    let trimmed = line.trim();

    // Check for various bullet formats
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("• ") {
        return Some(rest.trim().to_string());
    }

    // Numbered bullets like "1. " or "1) "
    if let Some(first_char) = trimmed.chars().next()
        && first_char.is_ascii_digit()
    {
        if let Some(dot_pos) = trimmed.find(". ") {
            return Some(trimmed[dot_pos + 2..].trim().to_string());
        }
        if let Some(paren_pos) = trimmed.find(") ") {
            return Some(trimmed[paren_pos + 2..].trim().to_string());
        }
    }

    None
}

/// Convert user stories to tasks.
pub fn stories_to_tasks(stories: &[UserStory]) -> Vec<Task> {
    stories
        .iter()
        .map(|story| {
            // Create a more descriptive task ID
            let id = story.id.to_lowercase().replace(' ', "-");

            Task {
                id,
                title: story.title.clone(),
                status: TaskStatus::Todo,
                acceptance: story.acceptance.clone(),
                validators: Vec::new(),
                notes: story.story.clone(),
            }
        })
        .collect()
}

/// Parse a PRD file and return tasks.
pub fn parse_prd_file(path: &Path) -> Result<Vec<Task>> {
    let content = std::fs::read_to_string(path)?;
    let stories = parse_prd(&content);
    Ok(stories_to_tasks(&stories))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_user_story_header_us_format() {
        let result = parse_user_story_header("### US-1: Continue and cook");
        assert!(result.is_some());
        let story = result.unwrap();
        assert_eq!(story.id, "US-1");
        assert_eq!(story.title, "Continue and cook");
    }

    #[test]
    fn test_parse_user_story_header_quoted() {
        let result = parse_user_story_header("### US-1: \"Continue and cook\"");
        assert!(result.is_some());
        let story = result.unwrap();
        assert_eq!(story.id, "US-1");
        assert_eq!(story.title, "Continue and cook");
    }

    #[test]
    fn test_parse_user_story_header_user_story_format() {
        let result = parse_user_story_header("### User Story 1: Continue and cook");
        assert!(result.is_some());
        let story = result.unwrap();
        assert_eq!(story.id, "US-1");
        assert_eq!(story.title, "Continue and cook");
    }

    #[test]
    fn test_parse_bullet_dash() {
        assert_eq!(
            parse_bullet("- criterion 1"),
            Some("criterion 1".to_string())
        );
    }

    #[test]
    fn test_parse_bullet_asterisk() {
        assert_eq!(
            parse_bullet("* criterion 2"),
            Some("criterion 2".to_string())
        );
    }

    #[test]
    fn test_parse_bullet_numbered() {
        assert_eq!(
            parse_bullet("1. criterion 3"),
            Some("criterion 3".to_string())
        );
    }

    #[test]
    fn test_parse_prd_simple() {
        let content = r#"
# PRD

## User Stories

### US-1: First feature
As a user, I want to do something.

Acceptance:
- Criterion 1
- Criterion 2

### US-2: Second feature
As a user, I want another thing.

Acceptance:
- Criterion A
- Criterion B
"#;

        let stories = parse_prd(content);
        assert_eq!(stories.len(), 2);

        assert_eq!(stories[0].id, "US-1");
        assert_eq!(stories[0].title, "First feature");
        assert_eq!(stories[0].acceptance.len(), 2);
        assert_eq!(stories[0].acceptance[0], "Criterion 1");
        assert_eq!(stories[0].acceptance[1], "Criterion 2");

        assert_eq!(stories[1].id, "US-2");
        assert_eq!(stories[1].title, "Second feature");
        assert_eq!(stories[1].acceptance.len(), 2);
    }

    #[test]
    fn test_parse_prd_with_story_text() {
        let content = r#"
### US-1: Test feature
As a user, I want to test something so that I can verify it works.

Acceptance:
- It works
"#;

        let stories = parse_prd(content);
        assert_eq!(stories.len(), 1);
        assert!(stories[0].story.is_some());
        assert!(stories[0].story.as_ref().unwrap().contains("As a user"));
    }

    #[test]
    fn test_stories_to_tasks() {
        let stories = vec![UserStory {
            id: "US-1".to_string(),
            title: "Test feature".to_string(),
            story: Some("As a user...".to_string()),
            acceptance: vec!["Criterion 1".to_string(), "Criterion 2".to_string()],
        }];

        let tasks = stories_to_tasks(&stories);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "us-1");
        assert_eq!(tasks[0].title, "Test feature");
        assert_eq!(tasks[0].status, TaskStatus::Todo);
        assert_eq!(tasks[0].acceptance.len(), 2);
        assert_eq!(tasks[0].notes, Some("As a user...".to_string()));
    }

    #[test]
    fn test_parse_actual_prd_format() {
        // Test with the actual format from product/prd.md
        let content = r#"
## 3. Core user stories

### US-1: "Continue and cook"
As a developer, when I run `ralpher continue` in a repo with `ralpher.toml`, I want ralpher to keep iterating until the PRD is fully done, so I can rely on a single command to drive work to completion.

Acceptance:
- Detects `ralpher.toml` and required artifacts (PRD file / task list)
- Starts a run (or resumes the last run) automatically
- Iterates until completion criteria met for all tasks
- Produces a final summary (what changed, last commit, validators)

### US-2: TUI observability
As a developer, I want a TUI that shows iteration count, last agent output, validator results, current task, and PRD doneness, so I can understand progress at a glance.

Acceptance:
- TUI shows: current phase, current task, task completion %, iterations elapsed, time elapsed, next action
- Live streaming logs (agent + validators)
- Clear indicators for PASS/FAIL/policy violations
- View last commit SHA / diff summary
"#;

        let stories = parse_prd(content);
        assert_eq!(stories.len(), 2);

        assert_eq!(stories[0].id, "US-1");
        assert_eq!(stories[0].title, "Continue and cook");
        assert_eq!(stories[0].acceptance.len(), 4);
        assert!(stories[0].acceptance[0].contains("Detects `ralpher.toml`"));

        assert_eq!(stories[1].id, "US-2");
        assert_eq!(stories[1].title, "TUI observability");
        assert_eq!(stories[1].acceptance.len(), 4);
    }
}

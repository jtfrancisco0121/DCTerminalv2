//! ClickUp's JSON, as far as intake needs it. Fields ClickUp may leave out
//! or send as `null` default, so a reply with less in it still reads. Ids
//! arrive as strings or numbers depending on the endpoint; both read as text.
//! Times are ClickUp's unix milliseconds, kept as sent.

use serde::{Deserialize, Deserializer};

/// A string, a number, or null, read as text (`""` for null).
fn id_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(text) => text,
        serde_json::Value::Number(number) => number.to_string(),
        _ => String::new(),
    })
}

fn opt_id_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let text = id_text(deserializer)?;
    Ok((!text.is_empty()).then_some(text))
}

/// `null` reads as the type's default, like a missing field.
fn null_default<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    deserializer: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// A workspace ("team" in API v2).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Team {
    #[serde(deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

/// `GET /team`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Teams {
    #[serde(default, deserialize_with = "null_default")]
    pub teams: Vec<Team>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct User {
    #[serde(default, deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub username: String,
    #[serde(default, deserialize_with = "null_default")]
    pub email: String,
}

/// A task's status, or one of a list's statuses.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Status {
    #[serde(default, deserialize_with = "null_default")]
    pub status: String,
    /// `open`, `custom`, `done` or `closed`.
    #[serde(default, rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    #[serde(default, deserialize_with = "null_default")]
    pub color: String,
    #[serde(default, deserialize_with = "null_default")]
    pub orderindex: serde_json::Value,
}

impl Status {
    /// Done and closed tickets are excluded from a round.
    pub fn is_closed(&self) -> bool {
        matches!(self.kind.as_str(), "closed" | "done")
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Priority {
    /// `urgent`, `high`, `normal` or `low`.
    #[serde(default, deserialize_with = "null_default")]
    pub priority: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Tag {
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ListRef {
    #[serde(default, deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Task {
    #[serde(deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "opt_id_text")]
    pub custom_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// The description as plain text.
    #[serde(default, deserialize_with = "null_default")]
    pub text_content: String,
    /// Only with `include_markdown_description=true`; see [`Task::description`].
    #[serde(default)]
    pub markdown_description: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub status: Status,
    /// `None` when the task has no priority.
    #[serde(default)]
    pub priority: Option<Priority>,
    #[serde(default, deserialize_with = "null_default")]
    pub tags: Vec<Tag>,
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default, deserialize_with = "opt_id_text")]
    pub parent: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub list: ListRef,
    #[serde(default, deserialize_with = "opt_id_text")]
    pub date_created: Option<String>,
    #[serde(default, deserialize_with = "opt_id_text")]
    pub date_updated: Option<String>,
    /// Only with `include_subtasks=true`.
    #[serde(default, deserialize_with = "null_default")]
    pub subtasks: Vec<Task>,
}

impl Task {
    /// The Markdown description, else the plain text one.
    pub fn description(&self) -> &str {
        self.markdown_description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or(&self.text_content)
    }
}

/// One comment. `comment_text` is ClickUp's plain-text rendering.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Comment {
    #[serde(deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub comment_text: String,
    #[serde(default, deserialize_with = "null_default")]
    pub user: User,
    /// Unix milliseconds.
    #[serde(default, deserialize_with = "id_text")]
    pub date: String,
}

/// `GET /task/{id}/comment`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Comments {
    #[serde(default, deserialize_with = "null_default")]
    pub comments: Vec<Comment>,
}

/// One page of `GET /list/{id}/task`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TaskPage {
    #[serde(default, deserialize_with = "null_default")]
    pub tasks: Vec<Task>,
    /// ClickUp sends this on newer replies; without it, a short page is last.
    #[serde(default)]
    pub last_page: Option<bool>,
}

/// `GET /list/{id}`: the list and the statuses a ticket can move to.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct List {
    #[serde(deserialize_with = "id_text")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub statuses: Vec<Status>,
}

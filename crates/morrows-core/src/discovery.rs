use crate::{Id, TaskState};
use chrono::{DateTime, Utc};
use serde::Serialize;

/// A bounded live result set. Follow next_offset with the same filters.
#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_offset: Option<i64>,
}

impl<T> Page<T> {
    /// Queries fetch one extra row to detect continuation without counting or
    /// materializing the entire history. Callers validate the page bounds first.
    pub fn from_extra_row(mut items: Vec<T>, limit: i64, offset: i64) -> Self {
        let has_more = items.len() > limit as usize;
        items.truncate(limit as usize);
        Self {
            items,
            next_offset: has_more.then_some(offset + limit),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: Id,
    pub project_id: Option<Id>,
    pub project_name: Option<String>,
    pub title: String,
    pub description_preview: String,
    pub description_truncated: bool,
    pub state: TaskState,
    pub priority: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct TaskQuery {
    pub agent_instance_id: Option<Id>,
    pub project_id: Option<Id>,
    pub state: Option<TaskState>,
    pub include_completed: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    pub id: Id,
    pub name: String,
    pub description_preview: String,
    pub description_truncated: bool,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

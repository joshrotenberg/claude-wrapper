//! Pluggable storage backend for pool state.
//!
//! The [`PoolStore`] trait abstracts where task and slot records live.
//! [`InMemoryStore`] keeps everything in-process; a future `RedisStore`
//! could share state across multiple pool server instances.

use async_trait::async_trait;
use dashmap::DashMap;

use crate::error::Result;
use crate::types::*;

/// Trait for storing and retrieving pool state.
///
/// Implementations must be `Send + Sync` for use in async contexts.
#[async_trait]
pub trait PoolStore: Send + Sync {
    /// Insert or update a task record.
    async fn put_task(&self, record: TaskRecord) -> Result<()>;

    /// Get a task by ID.
    async fn get_task(&self, id: &TaskId) -> Result<Option<TaskRecord>>;

    /// List tasks matching an optional filter.
    async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>>;

    /// Delete a task record.
    async fn delete_task(&self, id: &TaskId) -> Result<bool>;

    /// Insert or update a slot record.
    async fn put_slot(&self, record: SlotRecord) -> Result<()>;

    /// Get a slot by ID.
    async fn get_slot(&self, id: &SlotId) -> Result<Option<SlotRecord>>;

    /// List all slots.
    async fn list_slots(&self) -> Result<Vec<SlotRecord>>;

    /// Delete a slot record.
    async fn delete_slot(&self, id: &SlotId) -> Result<bool>;
}

/// In-memory store using [`DashMap`] for concurrent access.
///
/// All data is lost when the process exits. Suitable for single-session
/// usage and development.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    tasks: DashMap<String, TaskRecord>,
    slots: DashMap<String, SlotRecord>,
}

impl InMemoryStore {
    /// Create a new empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PoolStore for InMemoryStore {
    async fn put_task(&self, record: TaskRecord) -> Result<()> {
        self.tasks.insert(record.id.0.clone(), record);
        Ok(())
    }

    async fn get_task(&self, id: &TaskId) -> Result<Option<TaskRecord>> {
        Ok(self.tasks.get(&id.0).map(|r| r.value().clone()))
    }

    async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>> {
        let tasks: Vec<TaskRecord> = self
            .tasks
            .iter()
            .map(|r| r.value().clone())
            .filter(|t| {
                if let Some(state) = filter.state
                    && t.state != state
                {
                    return false;
                }
                if let Some(ref wid) = filter.slot_id
                    && t.slot_id.as_ref() != Some(wid)
                {
                    return false;
                }
                if let Some(ref tags) = filter.tags
                    && !tags.iter().any(|tag| t.tags.contains(tag))
                {
                    return false;
                }
                true
            })
            .collect();
        Ok(tasks)
    }

    async fn delete_task(&self, id: &TaskId) -> Result<bool> {
        Ok(self.tasks.remove(&id.0).is_some())
    }

    async fn put_slot(&self, record: SlotRecord) -> Result<()> {
        self.slots.insert(record.id.0.clone(), record);
        Ok(())
    }

    async fn get_slot(&self, id: &SlotId) -> Result<Option<SlotRecord>> {
        Ok(self.slots.get(&id.0).map(|r| r.value().clone()))
    }

    async fn list_slots(&self) -> Result<Vec<SlotRecord>> {
        Ok(self.slots.iter().map(|r| r.value().clone()).collect())
    }

    async fn delete_slot(&self, id: &SlotId) -> Result<bool> {
        Ok(self.slots.remove(&id.0).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_crud() {
        let store = InMemoryStore::new();
        let id = TaskId("t-1".into());

        let record = TaskRecord {
            id: id.clone(),
            prompt: "write tests".into(),
            state: TaskState::Pending,
            slot_id: None,
            result: None,
            tags: vec!["testing".into()],
            config: None,
            review_required: false,
            max_rejections: 3,
            rejection_count: 0,
            original_prompt: None,
            created_at_ms: None,
            started_at_ms: None,
            completed_at_ms: None,
        };

        store.put_task(record).await.unwrap();

        let fetched = store.get_task(&id).await.unwrap().unwrap();
        assert_eq!(fetched.prompt, "write tests");
        assert_eq!(fetched.state, TaskState::Pending);

        let all = store.list_tasks(&TaskFilter::default()).await.unwrap();
        assert_eq!(all.len(), 1);

        let deleted = store.delete_task(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get_task(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn slot_crud() {
        let store = InMemoryStore::new();
        let id = SlotId("w-0".into());

        let record = SlotRecord {
            id: id.clone(),
            state: SlotState::Idle,
            config: SlotConfig::default(),
            current_task: None,
            session_id: None,
            tasks_completed: 0,
            cost_microdollars: 0,
            restart_count: 0,
            worktree_path: None,
            mcp_config_path: None,
        };

        store.put_slot(record).await.unwrap();

        let fetched = store.get_slot(&id).await.unwrap().unwrap();
        assert_eq!(fetched.state, SlotState::Idle);

        let all = store.list_slots().await.unwrap();
        assert_eq!(all.len(), 1);

        let deleted = store.delete_slot(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get_slot(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn task_filter_by_state() {
        let store = InMemoryStore::new();

        for i in 0..3 {
            let state = if i == 0 {
                TaskState::Pending
            } else {
                TaskState::Completed
            };
            store
                .put_task(TaskRecord {
                    id: TaskId(format!("t-{i}")),
                    prompt: format!("task {i}"),
                    state,
                    slot_id: None,
                    result: None,
                    tags: vec![],
                    config: None,
                    review_required: false,
                    max_rejections: 3,
                    rejection_count: 0,
                    original_prompt: None,
                    created_at_ms: None,
                    started_at_ms: None,
                    completed_at_ms: None,
                })
                .await
                .unwrap();
        }

        let pending = store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::Pending),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);

        let completed = store
            .list_tasks(&TaskFilter {
                state: Some(TaskState::Completed),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(completed.len(), 2);
    }
}

//! Pluggable storage backend for pool state.
//!
//! The [`PoolStore`] trait abstracts where task and slot records live.
//! [`InMemoryStore`] keeps everything in-process; [`JsonFileStore`] persists
//! state to JSON files on disk so it survives restarts.

use std::path::{Path, PathBuf};

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

/// JSON file-backed store for durable pool state.
///
/// Tasks are stored as individual JSON files under `{dir}/tasks/{id}.json`
/// and slots under `{dir}/slots/{id}.json`. This makes state inspectable
/// with standard tools and survives process restarts.
#[derive(Debug)]
pub struct JsonFileStore {
    dir: PathBuf,
}

impl JsonFileStore {
    /// Create a new JSON file store rooted at `dir`.
    ///
    /// Creates the directory structure (`tasks/`, `slots/`) if it does not exist.
    pub async fn new(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        tokio::fs::create_dir_all(dir.join("tasks"))
            .await
            .map_err(|e| crate::Error::Store(format!("failed to create tasks dir: {e}")))?;
        tokio::fs::create_dir_all(dir.join("slots"))
            .await
            .map_err(|e| crate::Error::Store(format!("failed to create slots dir: {e}")))?;
        Ok(Self { dir })
    }

    /// Get the base directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn task_path(&self, id: &TaskId) -> PathBuf {
        self.dir.join("tasks").join(format!("{}.json", id.0))
    }

    fn slot_path(&self, id: &SlotId) -> PathBuf {
        self.dir.join("slots").join(format!("{}.json", id.0))
    }
}

#[async_trait]
impl PoolStore for JsonFileStore {
    async fn put_task(&self, record: TaskRecord) -> Result<()> {
        let path = self.task_path(&record.id);
        let json = serde_json::to_string_pretty(&record)?;
        tokio::fs::write(&path, json).await.map_err(|e| {
            crate::Error::Store(format!("failed to write task {}: {e}", path.display()))
        })?;
        Ok(())
    }

    async fn get_task(&self, id: &TaskId) -> Result<Option<TaskRecord>> {
        let path = self.task_path(id);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let record: TaskRecord = serde_json::from_str(&contents)?;
                Ok(Some(record))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(crate::Error::Store(format!(
                "failed to read task {}: {e}",
                path.display()
            ))),
        }
    }

    async fn list_tasks(&self, filter: &TaskFilter) -> Result<Vec<TaskRecord>> {
        let tasks_dir = self.dir.join("tasks");
        let mut entries = tokio::fs::read_dir(&tasks_dir)
            .await
            .map_err(|e| crate::Error::Store(format!("failed to read tasks dir: {e}")))?;

        let mut tasks = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| crate::Error::Store(format!("failed to read dir entry: {e}")))?
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
                crate::Error::Store(format!("failed to read {}: {e}", path.display()))
            })?;
            let record: TaskRecord = serde_json::from_str(&contents)?;

            // Apply filter.
            if let Some(state) = filter.state
                && record.state != state
            {
                continue;
            }
            if let Some(ref wid) = filter.slot_id
                && record.slot_id.as_ref() != Some(wid)
            {
                continue;
            }
            if let Some(ref tags) = filter.tags
                && !tags.iter().any(|tag| record.tags.contains(tag))
            {
                continue;
            }
            tasks.push(record);
        }

        Ok(tasks)
    }

    async fn delete_task(&self, id: &TaskId) -> Result<bool> {
        let path = self.task_path(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(crate::Error::Store(format!(
                "failed to delete task {}: {e}",
                path.display()
            ))),
        }
    }

    async fn put_slot(&self, record: SlotRecord) -> Result<()> {
        let path = self.slot_path(&record.id);
        let json = serde_json::to_string_pretty(&record)?;
        tokio::fs::write(&path, json).await.map_err(|e| {
            crate::Error::Store(format!("failed to write slot {}: {e}", path.display()))
        })?;
        Ok(())
    }

    async fn get_slot(&self, id: &SlotId) -> Result<Option<SlotRecord>> {
        let path = self.slot_path(id);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => {
                let record: SlotRecord = serde_json::from_str(&contents)?;
                Ok(Some(record))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(crate::Error::Store(format!(
                "failed to read slot {}: {e}",
                path.display()
            ))),
        }
    }

    async fn list_slots(&self) -> Result<Vec<SlotRecord>> {
        let slots_dir = self.dir.join("slots");
        let mut entries = tokio::fs::read_dir(&slots_dir)
            .await
            .map_err(|e| crate::Error::Store(format!("failed to read slots dir: {e}")))?;

        let mut slots = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| crate::Error::Store(format!("failed to read dir entry: {e}")))?
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let contents = tokio::fs::read_to_string(&path).await.map_err(|e| {
                crate::Error::Store(format!("failed to read {}: {e}", path.display()))
            })?;
            let record: SlotRecord = serde_json::from_str(&contents)?;
            slots.push(record);
        }

        Ok(slots)
    }

    async fn delete_slot(&self, id: &SlotId) -> Result<bool> {
        let path = self.slot_path(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(crate::Error::Store(format!(
                "failed to delete slot {}: {e}",
                path.display()
            ))),
        }
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

    // --- JsonFileStore tests ---

    fn make_task_record(id: &str, prompt: &str, state: TaskState) -> TaskRecord {
        TaskRecord {
            id: TaskId(id.into()),
            prompt: prompt.into(),
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
        }
    }

    fn make_slot_record(id: &str) -> SlotRecord {
        SlotRecord {
            id: SlotId(id.into()),
            state: SlotState::Idle,
            config: SlotConfig::default(),
            current_task: None,
            session_id: None,
            tasks_completed: 0,
            cost_microdollars: 0,
            restart_count: 0,
            worktree_path: None,
            mcp_config_path: None,
        }
    }

    #[tokio::test]
    async fn json_file_store_task_crud() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).await.unwrap();
        let id = TaskId("jfs-t-1".into());

        let record = make_task_record("jfs-t-1", "write tests", TaskState::Pending);
        store.put_task(record).await.unwrap();

        // File should exist on disk.
        assert!(store.task_path(&id).exists());

        let fetched = store.get_task(&id).await.unwrap().unwrap();
        assert_eq!(fetched.prompt, "write tests");
        assert_eq!(fetched.state, TaskState::Pending);

        let all = store.list_tasks(&TaskFilter::default()).await.unwrap();
        assert_eq!(all.len(), 1);

        let deleted = store.delete_task(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get_task(&id).await.unwrap().is_none());
        assert!(!store.task_path(&id).exists());
    }

    #[tokio::test]
    async fn json_file_store_slot_crud() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).await.unwrap();
        let id = SlotId("jfs-s-0".into());

        let record = make_slot_record("jfs-s-0");
        store.put_slot(record).await.unwrap();

        assert!(store.slot_path(&id).exists());

        let fetched = store.get_slot(&id).await.unwrap().unwrap();
        assert_eq!(fetched.state, SlotState::Idle);

        let all = store.list_slots().await.unwrap();
        assert_eq!(all.len(), 1);

        let deleted = store.delete_slot(&id).await.unwrap();
        assert!(deleted);
        assert!(store.get_slot(&id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn json_file_store_task_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).await.unwrap();

        store
            .put_task(make_task_record("jfs-f-0", "task 0", TaskState::Pending))
            .await
            .unwrap();
        store
            .put_task(make_task_record("jfs-f-1", "task 1", TaskState::Completed))
            .await
            .unwrap();
        store
            .put_task(make_task_record("jfs-f-2", "task 2", TaskState::Completed))
            .await
            .unwrap();

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

    #[tokio::test]
    async fn json_file_store_delete_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileStore::new(dir.path()).await.unwrap();

        let deleted = store.delete_task(&TaskId("nope".into())).await.unwrap();
        assert!(!deleted);

        let deleted = store.delete_slot(&SlotId("nope".into())).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn json_file_store_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Write with one store instance.
        {
            let store = JsonFileStore::new(dir.path()).await.unwrap();
            store
                .put_task(make_task_record(
                    "persist-1",
                    "durable task",
                    TaskState::Pending,
                ))
                .await
                .unwrap();
            store
                .put_slot(make_slot_record("persist-s-0"))
                .await
                .unwrap();
        }

        // Read with a fresh store instance.
        {
            let store = JsonFileStore::new(dir.path()).await.unwrap();
            let task = store
                .get_task(&TaskId("persist-1".into()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(task.prompt, "durable task");

            let slot = store
                .get_slot(&SlotId("persist-s-0".into()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(slot.state, SlotState::Idle);
        }
    }
}

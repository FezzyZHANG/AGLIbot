use super::*;

const TASKS: &str = "tasks";

impl MongoStore {
    pub async fn tasks(&self) -> Result<Vec<TaskRecord>> {
        collect(
            self.raw(TASKS)
                .find(doc! {})
                .sort(doc! { "created": 1, "_id": 1 })
                .await?,
            from_bson_doc,
        )
        .await
    }

    pub async fn task(&self, id: &str) -> Result<Option<TaskRecord>> {
        self.find(TASKS, id).await
    }

    pub async fn save_task(&self, task: &TaskRecord) -> Result<()> {
        self.save(TASKS, &task.id, task).await
    }

    // Compare the persisted snapshot before replacing it. Concurrent status and followup
    // updates retry against the winning version, including when timestamps share a millisecond.
    async fn mutate_task(
        &self,
        id: &str,
        change: impl Fn(&mut TaskRecord) -> Result<bool>,
    ) -> Result<Option<TaskRecord>> {
        let tasks = self.collection(TASKS);
        loop {
            let original = tasks
                .find_one(doc! { "_id": id })
                .await?
                .with_context(|| format!("没有这个任务：{id}"))?;
            let mut task: TaskRecord = from_bson_doc(original.clone())?;
            if !change(&mut task)? {
                return Ok(None);
            }
            let replacement = to_bson_doc(&task, Some(Bson::String(task.id.clone())))?;
            let filter = doc! {
                "_id": id,
                "$expr": { "$eq": ["$$ROOT", { "$literal": original }] },
            };
            let result = tasks.replace_one(filter, replacement).await?;
            if result.matched_count == 1 {
                return Ok(Some(task));
            }
        }
    }

    pub async fn append_task_followup(
        &self,
        id: &str,
        body: &str,
        thread: ThreadRef,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord> {
        self.mutate_task(id, |task| {
            task.append_followup(body, &thread, truncate_ms(now))?;
            Ok(true)
        })
        .await?
        .context("followup update skipped")
    }

    pub async fn mark_task_followups_consumed(
        &self,
        id: &str,
        up_to_seq: u64,
        round: u32,
    ) -> Result<()> {
        self.mutate_task(id, |task| {
            task.consume_followups(up_to_seq, round);
            Ok(true)
        })
        .await?;
        Ok(())
    }

    pub async fn set_task_status(
        &self,
        id: &str,
        status: TaskStatus,
        note: &str,
        now: DateTime<Utc>,
        verdict: Option<TaskVerdict>,
    ) -> Result<TaskRecord> {
        self.mutate_task(id, |task| {
            task.update_status(status, note, truncate_ms(now), verdict.clone());
            Ok(true)
        })
        .await?
        .context("status update skipped")
    }

    pub async fn recover_active_tasks(&self, now: DateTime<Utc>) -> Result<Vec<String>> {
        let active = self
            .raw(TASKS)
            .find(doc! { "status": "active" })
            .sort(doc! { "created": 1, "_id": 1 })
            .await?;
        let active: Vec<TaskRecord> = collect(active, from_bson_doc).await?;
        let mut recovered = Vec::new();
        for task in active {
            if self
                .mutate_task(&task.id, |task| {
                    if task.status != TaskStatus::Active {
                        return Ok(false);
                    }
                    task.update_status(TaskStatus::Inbox, RECOVERY_NOTE, truncate_ms(now), None);
                    Ok(true)
                })
                .await?
                .is_some()
            {
                recovered.push(task.id);
            }
        }
        Ok(recovered)
    }
}

//! 先持久化完整状态事务，再更新两个兼容视图；任何写入边界断电都能重放。
use super::*;

#[derive(Serialize, Deserialize)]
struct StateTransaction {
    task: Task,
    status: StatusRecord,
}

impl TaskStore {
    fn transaction_path(&self, id: &str) -> Result<PathBuf, TaskError> {
        Ok(self.task_dir(id)?.join("state-transaction.json"))
    }

    pub(super) fn commit_state(&self, task: &Task, status: &StatusRecord) -> Result<(), TaskError> {
        let transaction = StateTransaction {
            task: task.clone(),
            status: status.clone(),
        };
        write_json_atomic(self.transaction_path(&task.task_id)?, &transaction)?;
        self.replay_state_transaction(&task.task_id)
    }

    pub(super) fn replay_state_transaction(&self, id: &str) -> Result<(), TaskError> {
        let path = self.transaction_path(id)?;
        if !path.try_exists()? {
            return Ok(());
        }
        let transaction: StateTransaction = read_json(&path)?;
        let task = &transaction.task;
        let status = &transaction.status;
        task.validate()?;
        if canonical_task_id(&task.task_id)? != canonical_task_id(id)?
            || status.task_id != task.task_id
            || status.operation != task.operation
            || status.stage != task.status
        {
            return Err(TaskError::Invalid("状态事务身份或阶段不一致".into()));
        }
        // 事务只能改变阶段，不能换掉目标、镜像或已授权参数。
        let mut current: Task = read_json(self.task_path(id)?)?;
        if current.status != task.status && !current.status.can_transition_to(task.status) {
            return Err(TaskError::Invalid("状态事务存在非法阶段跳转".into()));
        }
        current.status = task.status;
        let mut expected = task.clone();
        for value in [&mut current, &mut expected] {
            // 盘符由已验证的 WinRE 挂载流程更新，不属于持久卷身份。
            if let Some(v) = value.source.as_mut() {
                v.drive_letter = None;
            }
            if let Some(v) = value.workspace_volume.as_mut() {
                v.drive_letter = None;
            }
            if let Some(v) = value.target.as_mut() {
                v.volume.drive_letter = None;
            }
            if let Some(v) = value.image.as_mut() {
                v.volume.drive_letter = None;
            }
            if let Some(v) = value.destination.as_mut() {
                v.volume.drive_letter = None;
            }
        }
        if current != expected {
            return Err(TaskError::Invalid("状态事务改变了任务身份或参数".into()));
        }
        write_json_atomic(self.task_path(id)?, task)?;
        write_json_atomic(self.status_path(id)?, status)?;
        fs::remove_file(path)?;
        Ok(())
    }
}

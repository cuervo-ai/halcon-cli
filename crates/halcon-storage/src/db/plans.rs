use uuid::Uuid;

use halcon_core::error::{HalconError, Result};

use super::Database;

/// A row from the planning_steps table.
#[derive(Debug, Clone)]
pub struct PlanStepRow {
    pub plan_id: String,
    pub step_index: u32,
    pub goal: String,
    pub description: String,
    pub tool_name: Option<String>,
    pub confidence: f64,
    pub outcome: Option<String>,
    pub outcome_detail: Option<String>,
}

impl Database {
    /// Persist an execution plan's steps to the planning_steps table.
    pub fn save_plan_steps(
        &self,
        session_id: &Uuid,
        plan: &halcon_core::traits::ExecutionPlan,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

        for (i, step) in plan.steps.iter().enumerate() {
            conn.execute(
                "INSERT INTO planning_steps (plan_id, parent_plan_id, session_id, goal, step_index, description, tool_name, confidence, replan_count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    plan.plan_id.to_string(),
                    plan.parent_plan_id.map(|id| id.to_string()),
                    session_id.to_string(),
                    plan.goal,
                    i as u32,
                    step.description,
                    step.tool_name,
                    step.confidence,
                    plan.replan_count,
                ],
            )
            .map_err(|e| HalconError::DatabaseError(format!("save plan step: {e}")))?;
        }

        Ok(())
    }

    /// Load plan steps by plan_id prefix (matches the first 8+ chars).
    pub fn load_plan_steps(&self, plan_id_prefix: &str) -> Result<Vec<PlanStepRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT plan_id, step_index, goal, description, tool_name, confidence, outcome, outcome_detail
                 FROM planning_steps
                 WHERE plan_id LIKE ?1
                 ORDER BY step_index ASC",
            )
            .map_err(|e| HalconError::DatabaseError(format!("prepare load_plan_steps: {e}")))?;

        let pattern = format!("{plan_id_prefix}%");
        let rows = stmt
            .query_map(rusqlite::params![pattern], |row| {
                Ok(PlanStepRow {
                    plan_id: row.get(0)?,
                    step_index: row.get(1)?,
                    goal: row.get(2)?,
                    description: row.get(3)?,
                    tool_name: row.get(4)?,
                    confidence: row.get(5)?,
                    outcome: row.get(6)?,
                    outcome_detail: row.get(7)?,
                })
            })
            .map_err(|e| HalconError::DatabaseError(format!("load plan steps: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }

    /// Update a plan step's outcome after execution.
    pub fn update_plan_step_outcome(
        &self,
        plan_id: &Uuid,
        step_index: u32,
        outcome: &str,
        outcome_detail: &str,
    ) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| HalconError::DatabaseError(e.to_string()))?;

        conn.execute(
            "UPDATE planning_steps SET outcome = ?1, outcome_detail = ?2
             WHERE plan_id = ?3 AND step_index = ?4",
            rusqlite::params![
                outcome,
                outcome_detail,
                plan_id.to_string(),
                step_index,
            ],
        )
        .map_err(|e| HalconError::DatabaseError(format!("update plan step outcome: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn test_plan(plan_id: Uuid) -> halcon_core::traits::ExecutionPlan {
        halcon_core::traits::ExecutionPlan {
            goal: "Fix the bug".into(),
            steps: vec![
                halcon_core::traits::PlanStep {
                    step_id: uuid::Uuid::new_v4(),
                    description: "Read the file".into(),
                    tool_name: Some("read_file".into()),
                    parallel: false,
                    confidence: 0.9,
                    expected_args: None,
                    outcome: None,
                },
                halcon_core::traits::PlanStep {
                    step_id: uuid::Uuid::new_v4(),
                    description: "Edit the file".into(),
                    tool_name: Some("edit_file".into()),
                    parallel: false,
                    confidence: 0.8,
                    expected_args: None,
                    outcome: None,
                },
            ],
            requires_confirmation: false,
            plan_id,
            replan_count: 0,
            parent_plan_id: None,
            ..Default::default()
        }
    }

    #[test]
    fn load_plan_steps_empty() {
        let db = test_db();
        let steps = db.load_plan_steps("nonexistent").unwrap();
        assert!(steps.is_empty());
    }

    #[test]
    fn load_plan_steps_by_prefix() {
        let db = test_db();
        let plan_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let plan = test_plan(plan_id);

        db.save_plan_steps(&session_id, &plan).unwrap();

        // Full UUID match.
        let steps = db.load_plan_steps(&plan_id.to_string()).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].description, "Read the file");
        assert_eq!(steps[1].description, "Edit the file");
        assert_eq!(steps[0].step_index, 0);
        assert_eq!(steps[1].step_index, 1);

        // Prefix match (first 8 chars).
        let prefix = &plan_id.to_string()[..8];
        let steps = db.load_plan_steps(prefix).unwrap();
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn load_plan_steps_ordered() {
        let db = test_db();
        let plan_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let plan = test_plan(plan_id);

        db.save_plan_steps(&session_id, &plan).unwrap();

        let steps = db.load_plan_steps(&plan_id.to_string()).unwrap();
        for (i, step) in steps.iter().enumerate() {
            assert_eq!(step.step_index, i as u32);
        }
    }

    #[test]
    fn load_plan_steps_with_outcome() {
        let db = test_db();
        let plan_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let plan = test_plan(plan_id);

        db.save_plan_steps(&session_id, &plan).unwrap();
        db.update_plan_step_outcome(&plan_id, 0, "success", "Completed OK").unwrap();

        let steps = db.load_plan_steps(&plan_id.to_string()).unwrap();
        assert_eq!(steps[0].outcome.as_deref(), Some("success"));
        assert_eq!(steps[0].outcome_detail.as_deref(), Some("Completed OK"));
        assert!(steps[1].outcome.is_none());
    }

    #[test]
    fn load_plan_steps_preserves_fields() {
        let db = test_db();
        let plan_id = Uuid::new_v4();
        let session_id = Uuid::new_v4();
        let plan = test_plan(plan_id);

        db.save_plan_steps(&session_id, &plan).unwrap();

        let steps = db.load_plan_steps(&plan_id.to_string()).unwrap();
        assert_eq!(steps[0].goal, "Fix the bug");
        assert_eq!(steps[0].tool_name.as_deref(), Some("read_file"));
        assert!((steps[0].confidence - 0.9).abs() < 0.01);
        assert_eq!(steps[1].tool_name.as_deref(), Some("edit_file"));
        assert!((steps[1].confidence - 0.8).abs() < 0.01);
    }
}

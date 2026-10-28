use std::collections::{BTreeMap, VecDeque};

use crate::{CheckPlanNode, CommandRun, CommandSummary, Error, ManifestPreview, Result, RunStatus};

pub(super) struct Task {
    pub command: CommandSummary,
    pub stage: Option<usize>,
    pub after: Option<usize>,
    pub budgets: Vec<usize>,
}

pub(super) struct Plan {
    pub tasks: Vec<Task>,
    pub budgets: Vec<usize>,
    pub stage_tasks: Vec<Vec<usize>>,
    pub stage_parents: Vec<Vec<usize>>,
    pub stage_order: Vec<usize>,
    pub prepare_count: usize,
    pub barriers: Vec<Vec<usize>>,
}

impl Plan {
    pub fn new(manifest: &ManifestPreview) -> Result<Self> {
        let mut plan = Self {
            tasks: Vec::new(),
            budgets: Vec::new(),
            stage_tasks: Vec::new(),
            stage_parents: Vec::new(),
            stage_order: Vec::new(),
            prepare_count: manifest.prepare.len(),
            barriers: Vec::new(),
        };
        for command in &manifest.prepare {
            let after = plan.tasks.len().checked_sub(1).map(|previous| {
                let barrier = plan.barriers.len();
                plan.barriers.push(vec![previous]);
                barrier
            });
            plan.tasks.push(Task {
                command: command.clone(),
                stage: None,
                after,
                budgets: Vec::new(),
            });
        }
        for (stage, summary) in manifest.stages.iter().enumerate() {
            let tasks = plan.sequence(&summary.check_plan, stage, None, &[]);
            plan.stage_tasks.push(tasks);
        }
        let indexes: BTreeMap<_, _> = manifest
            .stages
            .iter()
            .enumerate()
            .map(|(index, stage)| (stage.id.as_str(), index))
            .collect();
        let mut children = vec![Vec::new(); manifest.stages.len()];
        for (index, stage) in manifest.stages.iter().enumerate() {
            let mut parents = Vec::new();
            for parent in &stage.after {
                let parent = *indexes
                    .get(parent.as_str())
                    .ok_or_else(|| Error::execution("saved stage dependency is missing"))?;
                parents.push(parent);
                children[parent].push(index);
            }
            plan.stage_parents.push(parents);
        }
        let mut incoming: Vec<_> = plan.stage_parents.iter().map(Vec::len).collect();
        let mut ready: VecDeque<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        while let Some(stage) = ready.pop_front() {
            plan.stage_order.push(stage);
            for child in &children[stage] {
                incoming[*child] -= 1;
                if incoming[*child] == 0 {
                    ready.push_back(*child);
                }
            }
        }
        if plan.stage_order.len() != manifest.stages.len() {
            return Err(Error::execution(
                "saved stage dependency graph contains a cycle",
            ));
        }
        Ok(plan)
    }

    pub fn prerequisite(
        &self,
        task: usize,
        commands: &[CommandRun],
        cache: &mut [Option<RunStatus>],
    ) -> RunStatus {
        let Some(barrier) = self.tasks[task].after else {
            return RunStatus::Passed;
        };
        let members = &self.barriers[barrier];
        if members.len() == 1 {
            return commands[members[0]].status;
        }
        *cache[barrier].get_or_insert_with(|| {
            if members.iter().any(|index| {
                !commands[*index].status.is_active() && !commands[*index].status.is_success()
            }) {
                RunStatus::Failed
            } else if members
                .iter()
                .all(|index| commands[*index].status.is_success())
            {
                RunStatus::Passed
            } else {
                RunStatus::Pending
            }
        })
    }

    fn sequence(
        &mut self,
        nodes: &[CheckPlanNode],
        stage: usize,
        after: Option<usize>,
        budgets: &[usize],
    ) -> Vec<usize> {
        let mut previous = after;
        let mut all = Vec::new();
        for node in nodes {
            let members = self.node(node, stage, previous, budgets);
            all.extend_from_slice(&members);
            previous = Some(self.barriers.len());
            self.barriers.push(members);
        }
        all
    }

    fn node(
        &mut self,
        node: &CheckPlanNode,
        stage: usize,
        after: Option<usize>,
        budgets: &[usize],
    ) -> Vec<usize> {
        match node {
            CheckPlanNode::Command { command } => {
                let index = self.tasks.len();
                self.tasks.push(Task {
                    command: command.clone(),
                    stage: Some(stage),
                    after,
                    budgets: budgets.to_vec(),
                });
                vec![index]
            }

            CheckPlanNode::Sequence { steps, .. } => self.sequence(steps, stage, after, budgets),
            CheckPlanNode::Parallel {
                steps,
                max_parallel,
                ..
            } => {
                let mut budgets = budgets.to_vec();
                if let Some(limit) = max_parallel {
                    budgets.push(self.budgets.len());
                    self.budgets.push(limit.get() as usize);
                }
                steps
                    .iter()
                    .flat_map(|node| self.node(node, stage, after, &budgets))
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_sequential_sets_share_barriers_instead_of_a_quadratic_dependency_matrix() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("pipes.yaml");
        let mut yaml = "api: pipes/v1\napp: wide\nsource: {path: .}\nversion: {from: VERSION}\nstages:\n  - id: test\n    checks:\n".to_owned();
        for set in 0..2 {
            yaml.push_str(&format!("      - id: set-{set}\n        parallel:\n"));
            for command in 0..1000 {
                yaml.push_str(&format!(
                    "          - id: task-{set}-{command}\n            exec: [test-tool]\n"
                ));
            }
        }
        std::fs::write(&file, yaml).unwrap();
        let plan = Plan::new(&crate::preview_manifest(file).unwrap()).unwrap();
        assert_eq!(plan.tasks.len(), 2000);
        assert_eq!(plan.barriers.iter().map(Vec::len).sum::<usize>(), 2000);
        assert!(
            plan.tasks[1000..]
                .iter()
                .all(|task| task.after == plan.tasks[1000].after)
        );
    }
}

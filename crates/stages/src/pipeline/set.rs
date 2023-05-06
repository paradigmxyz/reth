use crate::{Stage, StageId};
use reth_db::database::Database;
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
};

/// Combines multiple [`Stage`]s into a single unit.
///
/// A [`StageSet`] is a logical chunk of stages that depend on each other. It is up to the
/// individual stage sets to determine what kind of configuration they expose.
///
/// Individual stages in the set can be added, removed and overridden using [`StageSetBuilder`].
pub trait StageSet<DB: Database>: Sized {
    /// Configures the stages in the set.
    fn builder(self) -> StageSetBuilder<DB>;

    /// Overrides the given [`Stage`], if it is in this set.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`] is not in this set.
    fn set<S: Stage<DB> + 'static>(self, stage: S) -> StageSetBuilder<DB> {
        self.builder().set(stage)
    }
}

struct StageEntry<DB> {
    stage: Box<dyn Stage<DB>>,
    enabled: bool,
}

impl<DB: Database> Debug for StageEntry<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageEntry")
            .field("stage", &self.stage.id())
            .field("enabled", &self.enabled)
            .finish()
    }
}

/// Helper to create and configure a [`StageSet`].
///
/// The builder provides ordering helpers to ensure that stages that depend on each other are added
/// to the final sync pipeline before/after their dependencies.
///
/// Stages inside the set can be disabled, enabled, overridden and reordered.
pub struct StageSetBuilder<DB> {
    stages: HashMap<StageId, StageEntry<DB>>,
    order: Vec<StageId>,
}

impl<DB: Database> Default for StageSetBuilder<DB> {
    fn default() -> Self {
        Self { stages: HashMap::new(), order: Vec::new() }
    }
}

impl<DB: Database> Debug for StageSetBuilder<DB> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageSetBuilder")
            .field("stages", &self.stages)
            .field("order", &self.order)
            .finish()
    }
}

impl<DB> StageSetBuilder<DB>
where
    DB: Database,
{
    fn index_of(&self, stage_id: StageId) -> usize {
        let index = self.order.iter().position(|&id| id == stage_id);

        index.unwrap_or_else(|| panic!("Stage does not exist in set: {stage_id}"))
    }

    fn upsert_stage_state(&mut self, stage: Box<dyn Stage<DB>>, added_at_index: usize) {
        let stage_id = stage.id();
        if self.stages.insert(stage.id(), StageEntry { stage, enabled: true }).is_some() {
            if let Some(to_remove) = self
                .order
                .iter()
                .enumerate()
                .find(|(i, id)| *i != added_at_index && **id == stage_id)
                .map(|(i, _)| i)
            {
                self.order.remove(to_remove);
            }
        }
    }

    /// Overrides the given [`Stage`], if it is in this set.
    ///
    /// # Panics
    ///
    /// Panics if the [`Stage`] is not in this set.
    pub fn set<S: Stage<DB> + 'static>(mut self, stage: S) -> Self {
        let entry = self
            .stages
            .get_mut(&stage.id())
            .unwrap_or_else(|| panic!("Stage does not exist in set: {}", stage.id()));
        entry.stage = Box::new(stage);
        self
    }

    /// Adds the given [`Stage`] at the end of this set.
    ///
    /// If the stage was already in the group, it is removed from its previous place.
    pub fn add_stage<S: Stage<DB> + 'static>(mut self, stage: S) -> Self {
        let target_index = self.order.len();
        self.order.push(stage.id());
        self.upsert_stage_state(Box::new(stage), target_index);
        self
    }

    /// Adds the given [`StageSet`] to the end of this set.
    ///
    /// If a stage is in both sets, it is removed from its previous place in this set. Because of
    /// this, it is advisable to merge sets first and re-order stages after if needed.
    pub fn add_set<Set: StageSet<DB>>(mut self, set: Set) -> Self {
        for stage in set.builder().build() {
            let target_index = self.order.len();
            self.order.push(stage.id());
            self.upsert_stage_state(stage, target_index);
        }
        self
    }

    /// Adds the given [`Stage`] before the stage with the given [`StageId`].
    ///
    /// If the stage was already in the group, it is removed from its previous place.
    ///
    /// # Panics
    ///
    /// Panics if the dependency stage is not in this set.
    pub fn add_before<S: Stage<DB> + 'static>(mut self, stage: S, before: StageId) -> Self {
        let target_index = self.index_of(before);
        self.order.insert(target_index, stage.id());
        self.upsert_stage_state(Box::new(stage), target_index);
        self
    }

    /// Adds the given [`Stage`] after the stage with the given [`StageId`].
    ///
    /// If the stage was already in the group, it is removed from its previous place.
    ///
    /// # Panics
    ///
    /// Panics if the dependency stage is not in this set.
    pub fn add_after<S: Stage<DB> + 'static>(mut self, stage: S, after: StageId) -> Self {
        let target_index = self.index_of(after) + 1;
        self.order.insert(target_index, stage.id());
        self.upsert_stage_state(Box::new(stage), target_index);
        self
    }

    /// Enables the given stage.
    ///
    /// All stages within a [`StageSet`] are enabled by default.
    ///
    /// # Panics
    ///
    /// Panics if the stage is not in this set.
    pub fn enable(mut self, stage_id: StageId) -> Self {
        let entry =
            self.stages.get_mut(&stage_id).expect("Cannot enable a stage that is not in the set.");
        entry.enabled = true;
        self
    }

    /// Disables the given stage.
    ///
    /// The disabled [`Stage`] keeps its place in the set, so it can be used for ordering with
    /// [`StageSetBuilder::add_before`] or [`StageSetBuilder::add_after`], or it can be re-enabled.
    ///
    /// All stages within a [`StageSet`] are enabled by default.
    ///
    /// # Panics
    ///
    /// Panics if the stage is not in this set.
    pub fn disable(mut self, stage_id: StageId) -> Self {
        let entry =
            self.stages.get_mut(&stage_id).expect("Cannot disable a stage that is not in the set.");
        entry.enabled = false;
        self
    }

    /// Disables the given stage if the given closure returns true.
    ///
    /// See [Self::disable]
    pub fn disable_if<F>(self, stage_id: StageId, f: F) -> Self
    where
        F: FnOnce() -> bool,
    {
        if f() {
            return self.disable(stage_id)
        }
        self
    }

    /// Consumes the builder and returns the contained [`Stage`]s in the order specified.
    pub fn build(mut self) -> Vec<Box<dyn Stage<DB>>> {
        let mut stages = Vec::new();
        for id in &self.order {
            if let Some(entry) = self.stages.remove(id) {
                if entry.enabled {
                    stages.push(entry.stage);
                }
            }
        }
        stages
    }
}

impl<DB: Database> StageSet<DB> for StageSetBuilder<DB> {
    fn builder(self) -> StageSetBuilder<DB> {
        self
    }
}

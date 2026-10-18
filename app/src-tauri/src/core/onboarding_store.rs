// Included within Store's module; shares the checked transaction implementation.
impl Store {
    pub fn onboarding(&self) -> StoreResult<OnboardingProgress> {
        decode(self.connection.query_row("SELECT data FROM onboarding WHERE singleton=1", [], |r| r.get(0))?)
    }
    pub fn save_onboarding(&mut self, mut next: OnboardingProgress, expected: u64) -> StoreResult<OnboardingProgress> {
        if next.completed_lessons.len() > 2 || (next.completed_lessons.len() == 2 && next.completed_lessons[0] == next.completed_lessons[1]) {
            return Err(invalid("invalid onboarding lessons"));
        }
        next.version = expected.checked_add(1).ok_or_else(|| invalid("version overflow"))?;
        let guard = self.commit_guard.clone();
        let tx = CheckedTransaction::new(&mut self.connection, self.health.clone())?;
        let changed = tx.execute("UPDATE onboarding SET version=?1,data=?2 WHERE singleton=1 AND version=?3",
            params![i64_checked(next.version)?, json(&next)?, i64_checked(expected)?])?;
        if changed != 1 {
            let current: u64 = tx.query_row("SELECT version FROM onboarding WHERE singleton=1", [], |r| r.get(0))?;
            return Err(StoreError::Conflict { current: current.to_string() });
        }
        tx.commit_guarded(guard.as_deref(), CommitKind::Revision, || {})?;
        Ok(next)
    }
}

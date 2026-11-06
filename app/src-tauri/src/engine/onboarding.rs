// Included in session.rs so onboarding shares the same capture/resource guards.
impl AppHub {
    pub fn onboarding_focus_changed(&self, focused: bool) {
        self.inner.lock().unwrap().onboarding.focused = focused;
    }

    pub fn onboarding_main_hidden(&self) {
        let show_recorder = {
            let mut i = self.inner.lock().unwrap();
            i.onboarding.focused = false;
            i.active.as_ref().is_some_and(|s| s.onboarding_lesson.is_some() && s.busy())
        };
        if show_recorder { let _ = self.native_action("recording", true); }
    }

    /// Applied before credentials or capture. An open guide never starts an
    /// unowned direct-insertion recording when the user switches applications.
    fn onboarding_start_lesson(&self, i: &Inner) -> Result<Option<OnboardingLesson>> {
        if !i.onboarding.active { return Ok(None); }
        if !i.onboarding.focused {
            return Err(ApiError::new("onboarding_focus_required", "onboarding", "请回到使用引导完成练习，或点开始使用后在其他应用口述"));
        }
        self.db.snapshot().onboarding.step.lesson().map(Some).ok_or_else(||ApiError::new("onboarding_step_required", "onboarding", "请先完成准备，再进入口述练习"))
    }

    fn onboarding_shortcut_check(&self) -> Result<bool> {
        let mut i = self.inner.lock().unwrap();
        if !i.onboarding.active || self.db.snapshot().onboarding.step != OnboardingStep::Shortcut { return Ok(false); }
        if !i.onboarding.focused {
            return Err(ApiError::new("onboarding_focus_required", "onboarding", "请回到使用引导测试快捷键"));
        }
        i.onboarding.shortcut_presses = i.onboarding.shortcut_presses.saturating_add(1);
        drop(i);
        self.changed();
        Ok(true)
    }

    fn onboarding_view(&self) -> Value {
        let i = self.inner.lock().unwrap();
        json!({"progress":self.db.snapshot().onboarding,"active":i.onboarding.active,"shortcutPresses":i.onboarding.shortcut_presses})
    }

    pub(super) async fn onboarding_command(self: &Arc<Self>, name: &str, payload: &Value) -> Result<Value> {
        let _reservation = self.reserve_settings()?;
        {
            let i = self.inner.lock().unwrap();
            if i.active.as_ref().is_some_and(ActiveSession::busy) || i.shortcut_capture.is_some() {
                return Err(ApiError::new("session_busy", "onboarding", "请先结束当前录音或快捷键设置，再切换引导步骤"));
            }
        }
        if name == "onboarding.open" {
            let mut progress = self.db.snapshot().onboarding;
            if progress.completed_at.is_some() && progress.step == OnboardingStep::Ready {
                let expected = progress.version;
                progress.step = OnboardingStep::Welcome;
                self.db_write(DbCommand::SaveOnboarding { progress, expected_version:expected }, self.operation_guard(), DbPriority::Normal).await?;
            }
            let mut i = self.inner.lock().unwrap();
            i.onboarding.active = true;
            i.onboarding.focused = true;
            i.onboarding.shortcut_presses = 0;
            drop(i);
            return Ok(self.onboarding_view());
        }
        if !self.inner.lock().unwrap().onboarding.active {
            return Err(ApiError::new("onboarding_inactive", "onboarding", "请先打开使用引导"));
        }
        let mut next = self.db.snapshot().onboarding;
        let expected = number(payload, "expectedVersion")?;
        match name {
            "onboarding.step" => {
                let step:OnboardingStep = parse(payload["step"].clone())?;
                if step == OnboardingStep::Ready && next.completed_lessons.len() != 2 {
                    return Err(ApiError::new("practice_required", "onboarding", "请先完成两次口述练习"));
                }
                next.step = step;
            }
            "onboarding.acceptLesson" => {
                let id = text(payload, "sessionId")?;
                let generation = number(payload, "expectedGeneration")?;
                let lesson = {
                    let i = self.inner.lock().unwrap();
                    let s = i.active.as_ref().filter(|s|s.id == id && s.generation == generation)
                        .ok_or_else(||ApiError::new("stale_session", "onboarding", "练习已变化，请核对当前结果"))?;
                    s.onboarding_lesson.filter(|lesson|next.step.lesson() == Some(*lesson)
                        && s.phase == SessionPhase::Ready && !s.busy() && s.view().worker_released
                        && s.first_samples && s.prefs.delivery_mode == DeliveryMode::Preview)
                        .ok_or_else(||ApiError::new("practice_required", "onboarding", "请先完成当前场景的一次真实口述"))?
                };
                let detail = db_detail(self.db_read(DbCommand::GetResult { session_id:id.into() }).await?)?;
                if detail.session.seed.generation != generation || !detail.source.complete
                    || detail.source.items.iter().all(|item|item.text.trim().is_empty())
                    || detail.output.as_ref().is_none_or(|output|output.blocks.iter().all(|block|block.text.trim().is_empty())) {
                    return Err(ApiError::new("practice_required", "onboarding", "请先完成口述并取得可查看的结果"));
                }
                let output=detail.output.as_ref().unwrap();
                if (output.rewrite_fallback.is_some() || output.rewrite_recovery.is_some() || output.kind==RevisionKind::UseSource)
                    && payload.get("acceptedFallbackRevisionId").and_then(Value::as_str)!=Some(output.id.as_str()) {
                    return Err(ApiError::new("source_confirmation_required","onboarding","整理尚未完成，请重试整理，或核对并确认当前识别原文后继续"));
                }
                if !next.completed_lessons.contains(&lesson) { next.completed_lessons.push(lesson); }
            }
            "onboarding.dismiss" => next.auto_show = false,
            "onboarding.finish" => {
                if ![OnboardingLesson::Chat, OnboardingLesson::Document].iter().all(|lesson|next.completed_lessons.contains(lesson)) {
                    return Err(ApiError::new("practice_required", "onboarding", "完成两次真实口述练习后即可结束引导，也可以选择稍后继续"));
                }
                next.auto_show = false;
                next.completed_at = Some(now());
                next.step = OnboardingStep::Ready;
            }
            _ => return Err(ApiError::new("invalid_command", "onboarding", "无效的引导操作")),
        }
        self.db_write(DbCommand::SaveOnboarding { progress:next, expected_version:expected }, self.operation_guard(), DbPriority::Normal).await?;
        if matches!(name, "onboarding.dismiss" | "onboarding.finish") {
            self.inner.lock().unwrap().onboarding.active = false;
        }
        Ok(self.onboarding_view())
    }
}

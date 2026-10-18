export type OnboardingStep = 'welcome' | 'setup' | 'shortcut' | 'chat' | 'document' | 'ready';
export type OnboardingLesson = 'chat' | 'document';
export interface OnboardingProgress {
  version: number;
  autoShow: boolean;
  step: OnboardingStep;
  completedLessons: OnboardingLesson[];
  completedAt: string | null;
}
export interface OnboardingState {
  progress: OnboardingProgress;
  active: boolean;
  shortcutPresses: number;
}
export const onboardingSteps: OnboardingStep[] = ['welcome', 'setup', 'shortcut', 'chat', 'document', 'ready'];

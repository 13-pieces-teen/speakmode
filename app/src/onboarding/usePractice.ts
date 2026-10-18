import {useCallback, useEffect, useRef, useState} from 'react';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import type {HistoryDetail} from '../services/contracts';
import type {OnboardingLesson} from './types';

export function usePractice(lesson: OnboardingLesson | null) {
  const desktop = useDesktop();
  const session = lesson && desktop.session?.onboardingLesson === lesson ? desktop.session : null;
  const id = session?.sessionId ?? session?.id;
  const generation = session?.generation;
  const phase = session?.state.toLowerCase();
  const recording = phase === 'starting' || phase === 'recording';
  const busy = !!session && (recording || session.workerReleased === false || ['finalizingasr','rewriting'].includes(phase ?? ''));
  const [loaded, setLoaded] = useState<{id:string; generation:number; detail:HistoryDetail} | null>(null);
  const [error, setError] = useState('');
  const [pending, setPending] = useState(false);
  const [read, setRead] = useState(0);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setError(''); }, [id, generation, lesson]);
  useEffect(() => {
    if (!id || generation === undefined || session?.workerReleased !== true || !session.sourceRevisionId) return;
    let current = true;
    void desktop.command<HistoryDetail>('history.get', {sessionId:id}).then(detail => {
      if (!current || detail.session.id !== id || detail.session.generation !== generation) return;
      setLoaded({id, generation, detail});
    }).catch(e => { if (current) setError(errorMessage(e)); });
    return () => { current = false; };
  }, [id, generation, phase, session?.workerReleased, session?.sourceRevisionId, desktop.command, read]);
  const perform = useCallback(async (name:string, extra:Record<string,unknown> = {}) => {
    if (inFlight.current) return;
    inFlight.current = true; setPending(true); setError('');
    try {
      await desktop.command(name, {sessionId:id, expectedGeneration:generation, failedAttemptId:session?.asrAttemptId, ...extra});
      await desktop.refresh();
      if (mounted.current) setRead(value => value + 1);
    } catch (e) { if (mounted.current) setError(errorMessage(e)); }
    finally { inFlight.current = false; if (mounted.current) setPending(false); }
  }, [desktop.command, desktop.refresh, id, generation, session?.asrAttemptId]);
  const detail = loaded && loaded.id === id && loaded.generation === generation ? loaded.detail : null;
  return {session, id, generation, recording, busy, detail, error, pending, perform, reload:() => setRead(value => value + 1)};
}

import {useCallback, useEffect, useRef, useState} from 'react';
import {useDesktop} from '../services/DesktopProvider';
import {errorMessage} from '../services/bridge';
import type {HistoryCandidate, HistoryDetail} from '../services/contracts';
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
  const [rewriting, setRewriting] = useState(false);
  const [read, setRead] = useState(0);
  const inFlight = useRef(false);
  const loadEpoch = useRef(0);
  const mounted = useRef(true);
  const owner = `${lesson}:${id}:${generation}`;
  const ownerRef = useRef(owner); ownerRef.current = owner;
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setError(''); }, [id, generation, lesson]);
  useEffect(() => {
    if (!id || generation === undefined || session?.workerReleased !== true || !session.sourceRevisionId) return;
    let current = true; const epoch = ++loadEpoch.current;
    void desktop.command<HistoryDetail>('history.get', {sessionId:id}).then(detail => {
      if (!current || epoch !== loadEpoch.current || detail.session.id !== id || detail.session.generation !== generation || detail.source.id !== session.sourceRevisionId) return;
      setLoaded({id, generation, detail});
    }).catch(e => { if (current && epoch === loadEpoch.current) setError(errorMessage(e)); });
    return () => { current = false; };
  }, [id, generation, phase, session?.workerReleased, session?.sourceRevisionId, desktop.command, read]);
  const detail = loaded && loaded.id === id && loaded.generation === generation ? loaded.detail : null;
  // Retry a saved fallback through the existing source-bound candidate/adoption
  // API. Never run the first-Ready commit twice or repeat native delivery.
  const retryRewrite = useCallback(async () => {
    if (inFlight.current || !detail?.output || !id || generation === undefined) return;
    const original = detail.output, runId = crypto.randomUUID(), expectedOwner = owner;
    const current = () => mounted.current && ownerRef.current === expectedOwner;
    inFlight.current = true; ++loadEpoch.current; setPending(true); setRewriting(true); setError('');
    let adopted = false;
    try {
      const candidate = await desktop.command<HistoryCandidate>('history.rewrite', {
        sessionId:id, sourceRevisionId:detail.source.id, baseRevisionId:original.id,
        modeId:original.modeSnapshot.modeId, runId,
      });
      if (!current()) return;
      if (candidate.sourceRevisionId !== detail.source.id || candidate.baseRevisionId !== original.id
          || candidate.sessionId !== id || candidate.reviewIssues?.length) {
        throw new Error('这次整理仍未通过完整检查，识别原文已保留。可再次重试，或核对原文后继续。');
      }
      const saved = await desktop.command<HistoryDetail>('revision.adopt', {
        sessionId:id, expectedCurrentRevisionId:original.id, candidateId:candidate.id,
      });
      adopted = true;
      if (current() && saved.session.id === id && saved.session.generation === generation
          && saved.source.id === detail.source.id) setLoaded({id,generation,detail:saved});
      await desktop.refresh();
    } catch (e) { if (current()) setError(errorMessage(e)); }
    finally {
      if (!adopted) await desktop.command('history.cancelCandidate',{sessionId:id,runId}).catch(() => {});
      inFlight.current = false;
      if (mounted.current) { setPending(false); setRewriting(false); }
    }
  }, [desktop.command, desktop.refresh, detail, id, generation, owner]);
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
  return {session, id, generation, recording, busy, detail, error, pending, rewriting, retryRewrite, perform, reload:() => setRead(value => value + 1)};
}

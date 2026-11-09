import type {RunRecord} from "@cowork/client";
export const finishedState=(state:RunRecord["state"])=>["completed","failed","canceled","expired"].includes(state);
/** Observe transitions only; opening an existing history never announces old results. */
export function finishedTransitions(previous:Map<string,RunRecord["state"]>,runs:RunRecord[]):RunRecord[]{
  const finished:RunRecord[]=[];
  for(const run of runs){const before=previous.get(run.id);if(before!==undefined&&!finishedState(before)&&finishedState(run.state))finished.push(run);previous.set(run.id,run.state);}
  return finished;
}

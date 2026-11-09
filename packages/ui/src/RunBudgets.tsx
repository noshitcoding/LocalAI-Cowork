import {useId} from "react";
import type {Budgets,RunDetail} from "@cowork/client";
import type {Key} from "./i18n";
import {useView} from "./view";
type T=(key:Key)=>string;
export function RunBudgets({value,setValue,disabled,t,contextTokens}:{value:Required<Budgets>,setValue:(value:Required<Budgets>)=>void,disabled:boolean,t:T,contextTokens:number|undefined}) {
  const id=useId();
  const language=useView(v=>v.language);
  const fields:[keyof Budgets,Key,number,number][]=[
    ["max_steps","maxRunSteps",1,4096],["max_seconds","maxRunSeconds",1,86400],
    ["max_output_tokens","maxReplyTokens",1,1000000],["max_total_tokens","maxRunTokens",1024,1000000000]
  ];
  return <details className="run-budgets"><summary>{t("runBudgets")}</summary><p>{t("budgetHint")}</p>
    <div className="budget-fields">{fields.map(([key,label,min,max])=><label key={key} htmlFor={`${id}-${key}`}>{t(label)}
      <input id={`${id}-${key}`} type="number" step={1} min={min} max={max} required disabled={disabled} value={value[key]} onChange={e=>setValue({...value,[key]:Number(e.target.value)})}/>
    </label>)}</div>{contextTokens!==undefined&&<p>{t("reservationPerRequest")}: {contextTokens.toLocaleString(language)}</p>}<p>{t("tokenReservationHint")}</p>
  </details>;
}
export function RunBudgetStatus({detail,t}:{detail:RunDetail|undefined,t:T}) {
  const language=useView(v=>v.language);
  if(!detail||detail.run.spec.file_action)return null;
  const budget=detail.token_budget;
  return <details className="run-budget-status"><summary>{t("runBudgetStatus")} · {budget.charged_tokens.toLocaleString(language)} / {budget.limit.toLocaleString(language)}</summary>
    <dl><dt>{t("remainingTokenBudget")}</dt><dd>{budget.remaining_tokens.toLocaleString(language)}</dd><dt>{t("unconfirmedTokenBudget")}</dt><dd>{budget.unconfirmed_tokens.toLocaleString(language)}</dd>
      <dt>{t("maxRunSteps")}</dt><dd>{detail.run.spec.budgets.max_steps}</dd><dt>{t("maxRunSeconds")}</dt><dd>{detail.run.spec.budgets.max_seconds}</dd><dt>{t("maxReplyTokens")}</dt><dd>{detail.run.spec.budgets.max_output_tokens}</dd></dl>
    {(detail.run.usage.incomplete||budget.unconfirmed_tokens>0)&&<p>{t("usageIncomplete")}</p>}
    {budget.limit_exceeded&&<p role="alert">{t("providerBudgetOverrun")}</p>}
  </details>;
}

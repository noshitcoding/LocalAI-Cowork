import type {Key} from "./i18n";
import {useMemo} from "react";
import {useQuery} from "@tanstack/react-query";
import type {PersonalMoneyStatus as MoneyStatusView} from "@cowork/client";
export type {PersonalMoneyStatus as MoneyStatusView} from "@cowork/client";
import {formatMoneyUnits, remainingMoneyUnits} from "./moneyFormatting";
type Language = "de" | "en";
type T = (key: Key) => string;

function display(status: Exclude<MoneyStatusView, {origin: "not_configured"}>, language: Language) {
  const pool = status.installation;
  const money = (value: string) => formatMoneyUnits(value, pool.currency, language);
  const amountKeys = ["limit_units", "charged_units", "confirmed_units", "pending_units", "unknown_units", "remaining_units"] as const;
  const amounts = Object.fromEntries(amountKeys.map(key => [key, money(pool[key])])) as Record<typeof amountKeys[number], string>;
  // An inconsistent response must never look like a verified free balance.
  for (const value of [pool.policy_version, pool.confirmed_attempts, pool.pending_attempts, pool.unknown_attempts]) remainingMoneyUnits("0", value);
  if (pool.policy_version === "0"
      || BigInt(pool.confirmed_units) + BigInt(pool.pending_units) + BigInt(pool.unknown_units) !== BigInt(pool.charged_units)
      || remainingMoneyUnits(pool.limit_units, pool.charged_units) !== pool.remaining_units
      || pool.limit_exceeded !== (BigInt(pool.charged_units) > BigInt(pool.limit_units))
      || pool.no_headroom !== (pool.remaining_units === "0")) throw new TypeError("Invalid monetary status");
  if ([[pool.confirmed_attempts, pool.confirmed_units], [pool.pending_attempts, pool.pending_units], [pool.unknown_attempts, pool.unknown_units]]
      .some(([count, amount]) => count === "0" && amount !== "0")) throw new TypeError("Invalid monetary status");
  if (status.origin === "operator_configuration") {
    money(status.defaults.project_limit_units);
    money(status.defaults.run_limit_units);
    remainingMoneyUnits("0", status.defaults.pool_policy_version);
    if (status.defaults.pool_policy_version === "0") throw new TypeError("Invalid monetary status");
  }
  const count = (value: string) => new Intl.NumberFormat(language === "de" ? "de-DE" : "en-US").format(BigInt(value));
  return {amounts, money, count};
}

export function MoneyStatusContent({status, language, t}: {status: MoneyStatusView; language: Language; t: T}) {
  if (status.origin === "not_configured") return <>
    <h3>{t("moneyNotConfigured")}</h3><p>{t("moneyNotConfiguredHint")}</p>
  </>;
  let shown: ReturnType<typeof display>;
  try { shown = display(status, language); }
  catch { return <p className="error" role="alert">{t("moneyUnavailable")}</p>; }
  const pool = status.installation;
  const rows: [Key, keyof typeof shown.amounts, string | undefined][] = [
    ["moneyLimit", "limit_units", undefined],
    ["moneyCharged", "charged_units", undefined],
    ["moneyConfirmed", "confirmed_units", pool.confirmed_attempts],
    ["moneyPending", "pending_units", pool.pending_attempts],
    ["moneyUnknown", "unknown_units", pool.unknown_attempts],
    ["moneyRemaining", "remaining_units", undefined],
  ];
  return <>
    <h3>{t(status.origin === "operator_configuration" ? "moneyConfigured" : "moneyLegacyManual")}</h3>
    {status.origin === "legacy_manual" && <p>{t("moneyLegacyHint")}</p>}
    <dl className="money-amounts">{rows.map(([label, amount, count]) => <div key={label}>
      <dt>{t(label)}</dt><dd>{shown.amounts[amount]}{count !== undefined && <small>{t("moneyRequests")}: {shown.count(count)}</small>}</dd>
    </div>)}</dl>
    {pool.pending_attempts !== "0" && <p>{t("moneyPendingHint")}</p>}
    {pool.unknown_attempts !== "0" && <p className="banner" role="status">{t("moneyUnknownHint")}</p>}
    {pool.overrun || pool.limit_exceeded ? <p className="banner" role="alert">{t("moneyOverrun")}</p>
      : pool.no_headroom && <p className="banner" role="status">{t("moneyExhausted")}</p>}
    {status.origin === "operator_configuration" && <section aria-label={t("moneyDefaults")}>
      <h4>{t("moneyDefaults")}</h4><dl className="money-amounts">
        <div><dt>{t("moneyProjectLimit")}</dt><dd>{shown.money(status.defaults.project_limit_units)}</dd></div>
        <div><dt>{t("moneyRunLimit")}</dt><dd>{shown.money(status.defaults.run_limit_units)}</dd></div>
      </dl>
    </section>}
    <p className="muted">{t("moneyHeadroomHint")}</p>
  </>;
}

export function PersonalMoneyStatus({load, language, t}: {load: () => Promise<MoneyStatusView>; language: Language; t: T}) {
  // A changed connection gets its own cache key without putting credentials in it.
  const scope = useMemo(() => crypto.randomUUID(), [load]);
  const query = useQuery({queryKey: ["personal-money", scope], queryFn: load, retry: false,
    refetchInterval: 5000, refetchOnWindowFocus: true});
  return <section className="personal-money">
    <p>{t("moneyBudgetHint")}</p>
    {query.isPending && <p role="status">{t("loading")}</p>}
    {query.isError && <p className="error" role="alert">{t("moneyUnavailable")}</p>}
    {query.isError && query.data && <p className="banner" role="status">{t("moneyLastSnapshot")}</p>}
    {query.data && <MoneyStatusContent status={query.data} language={language} t={t}/>}
    <div className="actions"><button type="button" disabled={query.isFetching} onClick={() => void query.refetch()}>{t("moneyRefresh")}</button></div>
  </section>;
}

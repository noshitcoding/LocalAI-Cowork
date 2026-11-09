import {describe, expect, it} from "vitest";
import {renderToStaticMarkup} from "react-dom/server";
import {MoneyStatusContent, type MoneyStatusView} from "./PersonalMoneyStatus";
import {de, en} from "./i18n";

function configured(): Extract<MoneyStatusView, {origin: "operator_configuration"}> {
  return {origin: "operator_configuration", installation: {
    currency: {code: "USD", scale: 9}, policy_version: "1", limit_units: "18446744073709551615",
    charged_units: "9007199254740993", confirmed_units: "9007199254740991", pending_units: "1", unknown_units: "1",
    remaining_units: "18437736874454810622", confirmed_attempts: "1", pending_attempts: "1", unknown_attempts: "1",
    overrun: false, limit_exceeded: false, no_headroom: false,
  }, defaults: {project_limit_units: "18446744073709551615", run_limit_units: "9007199254740993", pool_policy_version: "1"}};
}
function render(value: MoneyStatusView, language: "de" | "en" = "en") {
  const messages = language === "de" ? de : en;
  return renderToStaticMarkup(<MoneyStatusContent status={value} language={language} t={key => messages[key]}/>);
}

describe("personal installation money status", () => {
  it("preserves exact large and smallest-unit amounts in both languages", () => {
    const value = configured();
    const english = render(value), german = render(value, "de");
    expect(english).toContain("USD 18,446,744,073.709551615");
    expect(english).toContain("USD 9,007,199.254740993");
    expect(english).toContain("USD 0.000000001");
    expect(german).toContain("USD 18.446.744.073,709551615");
    expect(german).toContain("USD 0,000000001");
    expect(english).toContain(en.moneyUnknownHint);
    expect(german).toContain(de.moneyUnknownHint);
  });
  it("keeps pending and unconfirmed zero-price requests visible", () => {
    const value = configured();
    Object.assign(value.installation, {charged_units: "0", confirmed_units: "0", pending_units: "0", unknown_units: "0",
      remaining_units: value.installation.limit_units, confirmed_attempts: "0"});
    for (const language of ["de", "en"] as const) {
      const markup = render(value, language), messages = language === "de" ? de : en;
      expect(markup).toContain(messages.moneyPendingHint);
      expect(markup).toContain(messages.moneyUnknownHint);
      expect(markup).toContain(`${messages.moneyRequests}: 1`);
    }
  });
  it("shows legacy protection without invented automatic run defaults", () => {
    const value: MoneyStatusView = {origin: "legacy_manual", installation: configured().installation};
    const markup = render(value);
    expect(markup).toContain(en.moneyLegacyManual);
    expect(markup).toContain(en.moneyLegacyHint);
    expect(markup).not.toContain(en.moneyNotConfigured);
    expect(markup).not.toContain(en.moneyDefaults);
    expect(markup).not.toContain("<button");
  });
  it("distinguishes absent configuration from a zero charged balance", () => {
    const markup = render({origin: "not_configured"});
    expect(markup).toContain(en.moneyNotConfiguredHint);
    expect(markup).not.toContain(en.moneyRemaining);
    expect(markup).not.toContain(en.moneyLimit);
    expect(markup).not.toContain("USD");
  });
  it("shows exhausted headroom separately from a confirmed overrun", () => {
    const value = configured();
    Object.assign(value.installation, {limit_units: value.installation.charged_units, remaining_units: "0", no_headroom: true});
    expect(render(value)).toContain(en.moneyExhausted);
    expect(render(value)).not.toContain(en.moneyOverrun);
    value.installation.overrun = true;
    expect(render(value)).toContain(`<p class="banner" role="alert">${en.moneyOverrun}</p>`);
  });
  it("withholds balances for malformed or inconsistent status data", () => {
    const changes = [
      {remaining_units: "01"}, {remaining_units: "1"}, {confirmed_units: "9007199254740990"},
      {no_headroom: true}, {limit_exceeded: true}, {unknown_attempts: "0"}, {policy_version: "0"},
      {charged_units: 9007199254740992}, {unknown_units: "18446744073709551616"},
    ];
    for (const change of changes) {
      const value = configured(); Object.assign(value.installation, change);
      const markup = render(value);
      expect(markup).toContain(en.moneyUnavailable);
      expect(markup).not.toContain(en.moneyRemaining);
      expect(markup).not.toContain("USD");
      expect(markup).not.toContain(en.moneyConfigured);
    }
  });
});

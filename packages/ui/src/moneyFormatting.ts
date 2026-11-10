export type MoneyCurrency = {code: string; scale: number};

const maximumUnits = 18_446_744_073_709_551_615n;

function exactUnits(value: string): bigint {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]{0,19})$/.test(value)) {
    throw new TypeError("Invalid monetary amount");
  }
  const units = BigInt(value);
  if (units > maximumUnits) throw new TypeError("Invalid monetary amount");
  return units;
}

/** Decimal wire strings retain all u64 units, including values above 2^53. */
export function formatMoneyUnits(value: string, currency: MoneyCurrency, language: "de" | "en"): string {
  const units = exactUnits(value);
  if (!currency || typeof currency.code !== "string" || !/^[A-Z]{3}$/.test(currency.code)
      || !Number.isInteger(currency.scale) || currency.scale < 0 || currency.scale > 9
      || (language !== "de" && language !== "en")) {
    throw new TypeError("Invalid monetary currency");
  }
  const scale = 10n ** BigInt(currency.scale);
  const whole = new Intl.NumberFormat(language === "de" ? "de-DE" : "en-US").format(units / scale);
  if (currency.scale === 0) return `${currency.code} ${whole}`;
  // Trim only zero digits; never round a small hold or an uncertainty to zero.
  const minimumDigits = Math.min(2, currency.scale);
  let fraction = (units % scale).toString().padStart(currency.scale, "0");
  while (fraction.length > minimumDigits && fraction.endsWith("0")) fraction = fraction.slice(0, -1);
  return `${currency.code} ${whole}${language === "de" ? "," : "."}${fraction}`;
}

/** Display-only arithmetic; a server reservation still authorizes each attempt. */
export function remainingMoneyUnits(limit: string, charged: string): string {
  const ceiling = exactUnits(limit);
  const used = exactUnits(charged);
  return (used >= ceiling ? 0n : ceiling - used).toString();
}

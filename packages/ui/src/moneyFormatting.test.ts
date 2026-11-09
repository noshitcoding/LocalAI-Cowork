import {describe, expect, it} from "vitest";
import {formatMoneyUnits, remainingMoneyUnits} from "./moneyFormatting";

describe("exact monetary display", () => {
  it("retains every digit at the u64 boundary above Number's exact range", () => {
    expect(formatMoneyUnits("18446744073709551615", {code: "USD", scale: 9}, "en"))
      .toBe("USD 18,446,744,073.709551615");
    expect(formatMoneyUnits("9007199254740993", {code: "EUR", scale: 0}, "de"))
      .toBe("EUR 9.007.199.254.740.993");
  });
  it("keeps a single smallest unit visible rather than rounding it to zero", () => {
    expect(formatMoneyUnits("1", {code: "USD", scale: 9}, "en")).toBe("USD 0.000000001");
    expect(formatMoneyUnits("1", {code: "EUR", scale: 6}, "de")).toBe("EUR 0,000001");
    expect(formatMoneyUnits("1000000", {code: "EUR", scale: 6}, "de")).toBe("EUR 1,00");
  });
  it("uses the recorded scale even when it is smaller than two decimal places", () => {
    expect(formatMoneyUnits("1", {code: "USD", scale: 0}, "en")).toBe("USD 1");
    expect(formatMoneyUnits("1", {code: "USD", scale: 1}, "en")).toBe("USD 0.1");
  });
  it("rejects noncanonical or overflowing amounts instead of treating them as zero", () => {
    for (const amount of ["", "00", "01", "-1", "+1", "1.0", "1e3", " 1", "18446744073709551616"]) {
      expect(() => formatMoneyUnits(amount, {code: "USD", scale: 6}, "en")).toThrow(TypeError);
    }
    expect(() => formatMoneyUnits(9007199254740992 as unknown as string, {code: "USD", scale: 6}, "en"))
      .toThrow(TypeError);
  });
  it("rejects unsupported currency metadata without echoing input values", () => {
    for (const currency of [{code: "usd", scale: 6}, {code: "USD", scale: -1}, {code: "USD", scale: 10}, {code: "USD", scale: 1.5}]) {
      expect(() => formatMoneyUnits("1", currency, "en")).toThrow("Invalid monetary currency");
    }
  });
  it("subtracts exact large values and displays no negative headroom after overrun", () => {
    expect(remainingMoneyUnits("18446744073709551615", "18446744073709551614")).toBe("1");
    expect(remainingMoneyUnits("9007199254740993", "9007199254740992")).toBe("1");
    expect(remainingMoneyUnits("2", "3")).toBe("0");
    expect(() => remainingMoneyUnits("01", "0")).toThrow(TypeError);
  });
});

// @vitest-environment jsdom

import { describe, expect, it } from "vitest";

import "../src/shared/result-utils";

const resultUtils = (globalThis as any).StygianResultUtils as {
  normalizeExtractionEnvelope: (input: unknown) => {
    mode: "single" | "batch" | "unknown";
    rows: Record<string, unknown>[];
    metadata: Record<string, unknown>;
    errors: string[];
  };
  recordsToCsv: (rows: Record<string, unknown>[]) => string;
};

describe("result-utils", () => {
  it("normalizes a single-result MCP payload", () => {
    const payload = {
      result: {
        content: [
          {
            text: JSON.stringify({
              data: {
                full_name: "Ada Lovelace",
                company: "Analytical Engines",
              },
              metadata: {
                regions_successful: 2,
                total_regions: 2,
                elapsed_ms: 12,
              },
            }),
          },
        ],
      },
    };

    const normalized = resultUtils.normalizeExtractionEnvelope(payload);

    expect(normalized.mode).toBe("single");
    expect(normalized.rows).toEqual([
      { full_name: "Ada Lovelace", company: "Analytical Engines" },
    ]);
    expect(normalized.metadata.elapsed_ms).toBe(12);
  });

  it("normalizes a batch-result MCP payload and preserves row errors", () => {
    const payload = {
      result: {
        content: [
          {
            text: JSON.stringify({
              root_selector: "tbody > tr",
              total_matched: 2,
              successful: 1,
              results: [
                {
                  data: { full_name: "Grace Hopper", title: "Rear Admiral" },
                  successful_regions: 2,
                },
                {
                  error: "root row missing company column",
                  successful_regions: 0,
                },
              ],
            }),
          },
        ],
      },
    };

    const normalized = resultUtils.normalizeExtractionEnvelope(payload);

    expect(normalized.mode).toBe("batch");
    expect(normalized.rows).toEqual([
      { full_name: "Grace Hopper", title: "Rear Admiral" },
    ]);
    expect(normalized.metadata.root_selector).toBe("tbody > tr");
    expect(normalized.errors).toEqual(["root row missing company column"]);
  });

  it("exports a heterogeneous record set to CSV", () => {
    const csv = resultUtils.recordsToCsv([
      { full_name: "Ada Lovelace", company: "Analytical Engines" },
      {
        full_name: "Grace Hopper",
        title: "Rear Admiral",
        notes: "COBOL, Navy",
      },
    ]);

    expect(csv).toContain("full_name,company,title,notes");
    expect(csv).toContain("Ada Lovelace,Analytical Engines,,");
    expect(csv).toContain('Grace Hopper,,Rear Admiral,"COBOL, Navy"');
  });
});

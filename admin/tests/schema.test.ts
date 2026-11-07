import { describe, it, expect } from "vitest";
import seed from "../../assets/catalog/catalog-v1.json";
import { catalogSchema, appSchema } from "../src/features/catalog/schema";
describe("public classification contract", () => {
  it("accepts the complete migrated Windows/Mac catalog", () => {
    const d = catalogSchema.parse(seed);
    expect(d.folders).toHaveLength(52);
    expect(d.apps.length).toBeGreaterThan(200);
  });
  it("rejects empty rules and arbitrary icon URLs", () => {
    const a = structuredClone(seed.apps[0]);
    expect(appSchema.safeParse({ ...a, rules: [] }).success).toBe(false);
    expect(
      appSchema.safeParse({
        ...a,
        icon: { path: "https://example.com/a.png", source: "upload" },
      }).success,
    ).toBe(false);
  });
  it("requires schema compatibility", () => {
    expect(
      catalogSchema.safeParse({ ...seed, schema_version: 2 }).success,
    ).toBe(false);
  });
});

import { describe, expect, it } from "vitest";

import { initials, joinNames, phraseText, shortName } from "./people";
import { LONG } from "./scenarios";

describe("initials", () => {
  it("takes the first and last word's first letters", () => {
    expect(initials("Sam Rivera")).toBe("SR");
    expect(initials("Bartholomew Montgomery-Fitzgerald Junior")).toBe("BJ");
    expect(initials("kim")).toBe("K");
    expect(initials("  zoë   ångström ")).toBe("ZÅ");
    expect(initials("Иван Петров")).toBe("ИП");
  });

  it("uses one character for scripts without case", () => {
    expect(initials("王小明")).toBe("王");
    expect(initials("山田 太郎")).toBe("山");
    expect(initials("محمد الأحمد")).toBe("م");
  });

  it("keeps a character a person sees whole", () => {
    expect(initials("Éva Ńowak")).toBe("ÉŃ");
    expect(initials("émile")).toBe("É");
  });

  it("has something for an empty name", () => {
    expect(initials("")).toBe("?");
    expect(initials("   ")).toBe("?");
  });
});

describe("names in a line", () => {
  it("uses the first word", () => {
    expect(shortName("Sam Rivera")).toBe("Sam");
    expect(shortName("王小明")).toBe("王小明");
    expect(shortName(LONG.name)).toBe("Bartholomew");
    expect(shortName("")).toBe("");
  });

  it("joins names as a sentence does", () => {
    expect(joinNames([])).toBe("");
    expect(joinNames(["Sam"])).toBe("Sam");
    expect(joinNames(["Sam", "Kim"])).toBe("Sam and Kim");
    expect(joinNames(["Sam", "Kim", "Priya"])).toBe("Sam and 2 others");
  });

  it("reads a phrase as text", () => {
    expect(phraseText([{ name: "Sam" }, " is controlling"])).toBe("Sam is controlling");
  });

  it("checks the long name really is forty characters", () => {
    expect(LONG.name).toHaveLength(40);
  });
});

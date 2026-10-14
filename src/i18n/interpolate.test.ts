import { describe, expect, it } from "vitest";
import { interpolate } from "./index";

describe("interpolate", () => {
  it("replaces a single placeholder", () => {
    expect(interpolate("Hello {name}!", { name: "World" })).toBe("Hello World!");
  });

  it("replaces several placeholders", () => {
    expect(
      interpolate("{greeting}, {name}! You have {count} notifications.", {
        greeting: "Welcome",
        name: "User",
        count: "3",
      })
    ).toBe("Welcome, User! You have 3 notifications.");
  });

  it("handles numeric parameter values, including zero", () => {
    expect(interpolate("Episodio {number}", { number: 42 })).toBe("Episodio 42");
    expect(interpolate("Total: {count}", { count: 0 })).toBe("Total: 0");
  });

  it("leaves untouched any placeholder whose param is missing", () => {
    expect(interpolate("Hello {name}, your role is {role}", { name: "Alice" })).toBe(
      "Hello Alice, your role is {role}"
    );
  });

  it("returns the template as is when no params are provided", () => {
    expect(interpolate("Static text without parameters")).toBe("Static text without parameters");
    expect(interpolate("Template with {placeholder}")).toBe("Template with {placeholder}");
    expect(interpolate("Template with {placeholder}", undefined)).toBe(
      "Template with {placeholder}"
    );
  });

  it("replaces a placeholder repeated twice both times", () => {
    expect(interpolate("{token} matches {token}", { token: "abc" })).toBe("abc matches abc");
  });
});

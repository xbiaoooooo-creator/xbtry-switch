import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  isAppPage,
  isGlobalPage,
  parseView,
  readStoredView,
  storeView,
  VIEW_STORAGE_KEY,
} from "@/lib/navigation";

const workspaceVersionKey = "xbtry:workspace-version";
const formerModeKey = "xbtry:mode";

function resetNavigationStorage() {
  localStorage.removeItem(workspaceVersionKey);
  localStorage.removeItem(formerModeKey);
  localStorage.removeItem(VIEW_STORAGE_KEY);
}

describe("unified workspace navigation migration", () => {
  beforeEach(resetNavigationStorage);
  afterEach(() => {
    vi.restoreAllMocks();
    resetNavigationStorage();
  });

  it.each(["simple", "advanced"])(
    "opens the unified workspace and removes the former %s preference",
    (mode) => {
      localStorage.setItem(formerModeKey, mode);
      localStorage.setItem(VIEW_STORAGE_KEY, "providers");
      expect(readStoredView()).toBe("overview");
      expect(localStorage.getItem(formerModeKey)).toBeNull();
      expect(localStorage.getItem(workspaceVersionKey)).toBe("1");
    },
  );

  it("remains on the workspace when initialization reads the migrated view twice", () => {
    localStorage.setItem(formerModeKey, "simple");
    localStorage.setItem(VIEW_STORAGE_KEY, "usage");
    expect(readStoredView()).toBe("overview");
    // React StrictMode can invoke the App state initializer twice on first render.
    expect(readStoredView()).toBe("overview");
  });

  it("restores later user navigation without repeating the migration", () => {
    expect(readStoredView()).toBe("overview");
    storeView("sessions");
    expect(readStoredView()).toBe("sessions");
    storeView("overview");
    expect(readStoredView()).toBe("overview");
  });

  it("opens the workspace for a new installation or invalid stored view", () => {
    expect(readStoredView()).toBe("overview");
    localStorage.setItem(VIEW_STORAGE_KEY, "unknown-screen");
    expect(readStoredView()).toBe("overview");
    localStorage.removeItem(VIEW_STORAGE_KEY);
    expect(readStoredView()).toBe("overview");
  });

  it("keeps supported legacy app views restorable after migration", () => {
    localStorage.setItem(workspaceVersionKey, "1");
    localStorage.setItem(VIEW_STORAGE_KEY, "openclawEnv");
    expect(readStoredView()).toBe("openclawConfig");
  });

  it("still opens the workspace when local storage cannot be read", () => {
    vi.spyOn(localStorage, "getItem").mockImplementation(() => {
      throw new Error("Storage unavailable");
    });
    expect(readStoredView()).toBe("overview");
  });

  it("recognizes overview as a global destination", () => {
    expect(parseView("overview")).toBe("overview");
    expect(isGlobalPage("overview")).toBe(true);
    expect(isAppPage("overview")).toBe(false);
  });
});

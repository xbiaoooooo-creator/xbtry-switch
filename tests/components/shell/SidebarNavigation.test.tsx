import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Sidebar, type SidebarProps } from "@/components/shell/Sidebar";
import type { VisibleApps } from "@/types";
import { createTestQueryClient } from "../../utils/testQueryClient";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en" },
  }),
}));

vi.mock("@/contexts/UpdateContext", () => ({
  useUpdate: () => ({ hasUpdate: false }),
}));

vi.mock("@/hooks/useSidebarStatus", () => ({
  useSidebarStatus: () => ({
    appStatus: () => ({ mode: "direct", alert: false }),
    todayCost: 0,
    todayTokens: 0,
    todayRequests: 0,
    todayHitRate: 0,
    authNeedsAttention: false,
  }),
}));

function renderSidebar(overrides: Partial<SidebarProps> = {}) {
  const client = createTestQueryClient();
  client.setQueryDefaults(["settings"], { staleTime: Infinity });
  client.setQueryData(["settings"], {});
  const props: SidebarProps = {
    activeApp: "claude",
    view: "providers",
    visibleApps: { claude: true, codex: true, gemini: true } as VisibleApps,
    settingsSection: "general",
    onSelectApp: vi.fn(),
    onSelectPage: vi.fn(),
    onSelectSettingsSection: vi.fn(),
    onExitSettings: vi.fn(),
    onOpenXbtryImport: vi.fn(),
    ...overrides,
  };
  render(
    <QueryClientProvider client={client}>
      <Sidebar {...props} />
    </QueryClientProvider>,
  );
  return props;
}

describe("unified workspace navigation", () => {
  it("keeps the full workspace available for users with the old simple preference", async () => {
    localStorage.setItem("xbtry:mode", "simple");
    const props = renderSidebar();
    expect(screen.getByRole("button", { name: "nav.usage" })).toBeVisible();
    expect(screen.getByRole("button", { name: "MCP" })).toBeVisible();
    await userEvent.click(
      screen.getByRole("button", { name: "xbtry.overview" }),
    );
    expect(props.onSelectPage).toHaveBeenCalledWith("overview");
    await userEvent.click(
      screen.getByRole("button", { name: "xbtry.navImport" }),
    );
    expect(props.onOpenXbtryImport).toHaveBeenCalledOnce();
  });

  it("returns to the workspace from the brand logo", async () => {
    const props = renderSidebar({ view: "settings" });
    await userEvent.click(
      screen.getByRole("button", { name: "XBTry · xbtry.overview" }),
    );
    expect(props.onSelectPage).toHaveBeenCalledWith("overview");
  });

  it("keeps additional visible apps reachable in the expandable group", async () => {
    const props = renderSidebar();
    expect(screen.queryByRole("button", { name: "Gemini CLI" })).toBeNull();
    const toggle = screen.getByRole("button", { name: "xbtry.moreApps" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    await userEvent.click(screen.getByRole("button", { name: "Gemini CLI" }));
    expect(props.onSelectApp).toHaveBeenCalledWith("gemini");
    expect(screen.queryByRole("button", { name: "OpenCode" })).toBeNull();
  });

  it("opens the additional apps group when restoring an app in that group", () => {
    renderSidebar({ activeApp: "gemini" });
    expect(screen.getByRole("button", { name: "Gemini CLI" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(
      screen.getByRole("button", { name: "xbtry.moreApps" }),
    ).toHaveAttribute("aria-expanded", "true");
  });
});

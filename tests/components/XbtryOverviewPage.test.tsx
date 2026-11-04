import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { XbtryOverviewPage } from "@/components/xbtry/XbtryOverviewPage";
import type { Provider, VisibleApps } from "@/types";
import { createTestQueryClient } from "../utils/testQueryClient";

const api = vi.hoisted(() => ({ getAll: vi.fn(), getCurrent: vi.fn() }));

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  providersApi: api,
}));

const provider = (id: string, name: string): Provider => ({
  id,
  name,
  settingsConfig: {},
});

function renderOverview({
  visibleApps = { claude: true, codex: true } as VisibleApps,
  cachedClaude,
}: {
  visibleApps?: VisibleApps;
  cachedClaude?: {
    providers: Record<string, Provider>;
    currentProviderId: string;
  };
} = {}) {
  const client = createTestQueryClient();
  if (cachedClaude) client.setQueryData(["providers", "claude"], cachedClaude);
  const actions = {
    onOpenApp: vi.fn(),
    onOpenPage: vi.fn(),
    onImport: vi.fn(),
  };
  render(
    <QueryClientProvider client={client}>
      <XbtryOverviewPage visibleApps={visibleApps} {...actions} />
    </QueryClientProvider>,
  );
  return actions;
}

function card(name: "Claude Code" | "Codex") {
  const article = screen.getByRole("heading", { name }).closest("article");
  if (!article) throw new Error(`Missing connection card for ${name}`);
  return within(article);
}

describe("XbtryOverviewPage", () => {
  beforeEach(() => {
    api.getAll.mockReset().mockResolvedValue({});
    api.getCurrent.mockReset().mockResolvedValue("");
  });

  it("shows the selected configuration returned by each application", async () => {
    api.getAll.mockImplementation(async (app: string) =>
      app === "claude"
        ? {
            old: provider("old", "Previous connection"),
            selected: provider("selected", "Team Claude endpoint"),
          }
        : { personal: provider("personal", "Personal Codex endpoint") },
    );
    api.getCurrent.mockImplementation(async (app: string) =>
      app === "claude" ? "selected" : "personal",
    );
    renderOverview();

    expect(
      await card("Claude Code").findByText("Team Claude endpoint"),
    ).toBeVisible();
    expect(
      await card("Codex").findByText("Personal Codex endpoint"),
    ).toBeVisible();
    expect(card("Claude Code").getByText("workspace.configured")).toBeVisible();
    expect(card("Codex").getByText("workspace.configured")).toBeVisible();
    expect(screen.queryByText("Previous connection")).not.toBeInTheDocument();
    expect(api.getAll).toHaveBeenCalledWith("claude");
    expect(api.getCurrent).toHaveBeenCalledWith("codex");
  });

  it("does not claim a saved but inactive configuration is configured", async () => {
    api.getAll.mockResolvedValue({
      saved: provider("saved", "Saved connection"),
    });
    api.getCurrent.mockResolvedValue("removed-provider");
    renderOverview({ visibleApps: { claude: true } as VisibleApps });

    expect(
      await card("Claude Code").findByText("workspace.notConfigured"),
    ).toBeVisible();
    expect(
      card("Claude Code").getByText("workspace.noConfiguration"),
    ).toBeVisible();
    expect(screen.queryByText("workspace.configured")).not.toBeInTheDocument();
  });

  it("shows loading while provider data is still being read", () => {
    api.getAll.mockReturnValue(new Promise(() => {}));
    renderOverview({ visibleApps: { claude: true } as VisibleApps });

    expect(card("Claude Code").getAllByText("workspace.loading")).toHaveLength(
      2,
    );
    expect(screen.queryByText("workspace.configured")).not.toBeInTheDocument();
    expect(
      screen.queryByText("workspace.notConfigured"),
    ).not.toBeInTheDocument();
  });

  it("replaces stale configured status with a read error, then allows retry", async () => {
    api.getAll.mockRejectedValue(new Error("Configuration read failed"));
    api.getCurrent.mockResolvedValue("selected");
    renderOverview({
      visibleApps: { claude: true } as VisibleApps,
      cachedClaude: {
        providers: { selected: provider("selected", "Cached connection") },
        currentProviderId: "selected",
      },
    });

    expect(
      await card("Claude Code").findByText("workspace.readError"),
    ).toBeVisible();
    expect(
      card("Claude Code").getByText("workspace.readErrorHint"),
    ).toBeVisible();
    expect(screen.queryByText("workspace.configured")).not.toBeInTheDocument();
    expect(
      screen.queryByText("workspace.notConfigured"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Cached connection")).not.toBeInTheDocument();

    api.getAll.mockResolvedValue({
      selected: provider("selected", "Recovered connection"),
    });
    await userEvent.click(
      card("Claude Code").getByRole("button", { name: "workspace.retry" }),
    );
    expect(
      await card("Claude Code").findByText("Recovered connection"),
    ).toBeVisible();
    expect(card("Claude Code").getByText("workspace.configured")).toBeVisible();
  });

  it("treats a failed current-provider read as an error even when the list succeeds", async () => {
    api.getAll.mockResolvedValue({
      saved: provider("saved", "Saved connection"),
    });
    api.getCurrent.mockRejectedValue(new Error("Current provider unavailable"));
    renderOverview({ visibleApps: { codex: true } as VisibleApps });

    expect(await card("Codex").findByText("workspace.readError")).toBeVisible();
    expect(screen.queryByText("workspace.configured")).not.toBeInTheDocument();
    expect(
      screen.queryByText("workspace.notConfigured"),
    ).not.toBeInTheDocument();
  });

  it("opens the correct application and imports from the empty and global entry points", async () => {
    api.getAll.mockImplementation(async (app: string) =>
      app === "claude"
        ? { selected: provider("selected", "Team endpoint") }
        : {},
    );
    api.getCurrent.mockImplementation(async (app: string) =>
      app === "claude" ? "selected" : "",
    );
    const actions = renderOverview();

    await card("Claude Code").findByText("Team endpoint");
    await card("Codex").findByText("workspace.notConfigured");
    await userEvent.click(
      card("Claude Code").getByRole("button", {
        name: "workspace.manageConnection",
      }),
    );
    expect(actions.onOpenApp).toHaveBeenCalledWith("claude");
    await userEvent.click(
      card("Codex").getByRole("button", { name: "workspace.connect" }),
    );
    await userEvent.click(
      screen.getAllByRole("button", { name: "workspace.connect" }).at(-1)!,
    );
    expect(actions.onImport).toHaveBeenCalledTimes(2);
    await userEvent.click(
      screen.getByRole("button", { name: "workspace.allApps" }),
    );
    expect(actions.onOpenPage).toHaveBeenCalledWith("apps");
  });

  it("respects a hidden application and never queries its configuration", async () => {
    renderOverview({
      visibleApps: { claude: false, codex: true } as VisibleApps,
    });

    await card("Codex").findByText("workspace.notConfigured");
    expect(
      screen.queryByRole("heading", { name: "Claude Code" }),
    ).not.toBeInTheDocument();
    expect(api.getAll).not.toHaveBeenCalledWith("claude");
    expect(api.getCurrent).not.toHaveBeenCalledWith("claude");
  });

  it("offers app management when both primary applications are hidden", async () => {
    const actions = renderOverview({ visibleApps: {} as VisibleApps });

    expect(screen.getByText("workspace.noApps")).toBeVisible();
    expect(api.getAll).not.toHaveBeenCalled();
    expect(api.getCurrent).not.toHaveBeenCalled();
    await userEvent.click(
      screen.getAllByRole("button", { name: "workspace.allApps" })[1],
    );
    expect(actions.onOpenPage).toHaveBeenCalledWith("apps");
  });

  it.each([
    ["workspace.usage", "usage"],
    ["workspace.sessions", "sessions"],
    ["workspace.extensions", "mcp"],
  ])("opens the %s workspace shortcut", async (label, page) => {
    const actions = renderOverview({ visibleApps: {} as VisibleApps });
    await userEvent.click(
      screen.getByRole("button", { name: new RegExp(label) }),
    );
    expect(actions.onOpenPage).toHaveBeenCalledWith(page);
  });
});

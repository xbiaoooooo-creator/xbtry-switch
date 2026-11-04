import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { XbtryImportDialog } from "@/components/xbtry/XbtryImportDialog";
import { BRAND } from "@/config/brand";

const api = vi.hoisted(() => ({
  getAll: vi.fn(),
  getCurrent: vi.fn(),
  update: vi.fn(),
  switch: vi.fn(),
  updateTrayMenu: vi.fn(),
  importFromDeeplink: vi.fn(),
}));

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  providersApi: {
    getAll: api.getAll,
    getCurrent: api.getCurrent,
    update: api.update,
    switch: api.switch,
    updateTrayMenu: api.updateTrayMenu,
  },
  settingsApi: { openExternal: vi.fn() },
}));
vi.mock("@/lib/api/deeplink", () => ({
  deeplinkApi: { importFromDeeplink: api.importFromDeeplink },
}));

const renderDialog = () => {
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <XbtryImportDialog open onOpenChange={onOpenChange} />
    </QueryClientProvider>,
  );
  return { onOpenChange };
};

describe("XbtryImportDialog", () => {
  beforeEach(() => {
    Object.values(api).forEach((fn) => fn.mockReset());
    api.getAll.mockResolvedValue({});
    api.getCurrent.mockResolvedValue("");
    localStorage.removeItem("xbtry:mode");
  });

  afterEach(() => localStorage.removeItem("xbtry:mode"));

  it("imports a new XBTry provider into Claude Code by default", async () => {
    const { onOpenChange } = renderDialog();
    await userEvent.type(
      screen.getByLabelText("xbtry.apiKey"),
      "  sk-test-123 ",
    );
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));

    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(api.importFromDeeplink).toHaveBeenCalledTimes(1);
    expect(api.importFromDeeplink).toHaveBeenCalledWith(
      expect.objectContaining({
        app: "claude",
        name: BRAND.name,
        endpoint: BRAND.claude.baseUrl,
        apiKey: "sk-test-123",
        apiFormat: "anthropic",
        enabled: true,
      }),
    );
  });

  it("imports into Codex when its card is selected", async () => {
    renderDialog();
    await userEvent.click(
      screen.getByRole("checkbox", { name: "Claude Code" }),
    );
    await userEvent.click(screen.getByRole("checkbox", { name: "Codex" }));
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk-cx");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));

    await waitFor(() =>
      expect(api.importFromDeeplink).toHaveBeenCalledWith(
        expect.objectContaining({
          app: "codex",
          endpoint: BRAND.codex.baseUrl,
          model: BRAND.codex.model,
          apiFormat: "openai_responses",
        }),
      ),
    );
  });

  it("replaces the key on an existing XBTry row instead of adding another", async () => {
    api.getAll.mockImplementation(async (app: string) =>
      app === "claude"
        ? {
            x1: {
              id: "x1",
              name: "My relay",
              settingsConfig: {
                env: {
                  ANTHROPIC_BASE_URL: BRAND.claude.baseUrl,
                  ANTHROPIC_AUTH_TOKEN: "old",
                },
              },
            },
          }
        : {},
    );
    api.getCurrent.mockResolvedValue("other");
    renderDialog();
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk-new");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));

    await waitFor(() =>
      expect(api.switch).toHaveBeenCalledWith("x1", "claude"),
    );
    expect(api.importFromDeeplink).not.toHaveBeenCalled();
    expect(api.update).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "x1",
        settingsConfig: {
          env: {
            ANTHROPIC_BASE_URL: BRAND.claude.baseUrl,
            ANTHROPIC_AUTH_TOKEN: "sk-new",
          },
        },
      }),
      "claude",
    );
  });

  it("rejects keys containing whitespace", async () => {
    renderDialog();
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk a");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));
    expect(await screen.findByText("xbtry.keyInvalid")).toBeTruthy();
    expect(api.importFromDeeplink).not.toHaveBeenCalled();
  });

  it.each(["simple", "advanced"])(
    "offers the same controls and masked key with a saved %s preference",
    async (savedMode) => {
      localStorage.setItem("xbtry:mode", savedMode);
      renderDialog();
      expect(screen.queryByRole("radio")).not.toBeInTheDocument();
      expect(screen.getAllByRole("checkbox")).toHaveLength(2);
      expect(
        screen.getByRole("switch", { name: "xbtry.activate" }),
      ).toBeChecked();
      const input = screen.getByLabelText("xbtry.apiKey");
      expect(input).toHaveAttribute("type", "password");
      await userEvent.type(input, "sk-masked");
      await userEvent.click(
        screen.getByRole("button", { name: "xbtry.showKey" }),
      );
      expect(input).toHaveAttribute("type", "text");
      await userEvent.click(
        screen.getByRole("button", { name: "xbtry.hideKey" }),
      );
      expect(input).toHaveAttribute("type", "password");
      expect(input).toHaveValue("sk-masked");
    },
  );

  it("imports both selected tools without activating when the switch is off", async () => {
    const { onOpenChange } = renderDialog();
    await userEvent.click(screen.getByRole("checkbox", { name: "Codex" }));
    await userEvent.click(
      screen.getByRole("switch", { name: "xbtry.activate" }),
    );
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk-both");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(api.importFromDeeplink).toHaveBeenCalledTimes(2);
    for (const app of ["claude", "codex"]) {
      expect(api.importFromDeeplink).toHaveBeenCalledWith(
        expect.objectContaining({ app, apiKey: "sk-both", enabled: false }),
      );
    }
    expect(screen.getByLabelText("xbtry.apiKey")).toHaveValue("");
  });

  it("requires at least one selected tool", async () => {
    renderDialog();
    await userEvent.click(
      screen.getByRole("checkbox", { name: "Claude Code" }),
    );
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk-none");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));
    expect(await screen.findByText("xbtry.targetRequired")).toBeInTheDocument();
    expect(api.importFromDeeplink).not.toHaveBeenCalled();
  });

  it("keeps the key and retries only the failed target after partial success", async () => {
    const errorLog = vi.spyOn(console, "error").mockImplementation(() => {});
    api.importFromDeeplink.mockImplementation(
      async ({ app }: { app: string }) => {
        if (app === "codex") throw new Error("Cannot write Codex config");
      },
    );
    const { onOpenChange } = renderDialog();
    await userEvent.click(screen.getByRole("checkbox", { name: "Codex" }));
    await userEvent.type(screen.getByLabelText("xbtry.apiKey"), "sk-retry");
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Codex");
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(api.importFromDeeplink).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("xbtry.apiKey")).toHaveValue("sk-retry");
    expect(
      screen.getByRole("checkbox", { name: "Claude Code" }),
    ).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Codex" })).toBeChecked();

    api.importFromDeeplink.mockResolvedValue(undefined);
    await userEvent.click(screen.getByRole("button", { name: "xbtry.import" }));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(api.importFromDeeplink).toHaveBeenCalledTimes(3);
    expect(api.importFromDeeplink).toHaveBeenLastCalledWith(
      expect.objectContaining({ app: "codex", apiKey: "sk-retry" }),
    );
    errorLog.mockRestore();
  });
});

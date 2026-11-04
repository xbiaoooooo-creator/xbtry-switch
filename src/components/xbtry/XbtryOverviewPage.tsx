import { useQuery } from "@tanstack/react-query";
import {
  ArrowRight,
  BarChart3,
  Check,
  ChevronRight,
  KeyRound,
  Loader2,
  MessageSquare,
  Puzzle,
  RefreshCw,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { AppPageHeader } from "@/components/shell/AppPageHeader";
import { APP_DISPLAY_NAME, AppGlyph } from "@/components/shell/AppGlyph";
import { Button } from "@/components/ui/button";
import { providersApi, type AppId } from "@/lib/api";
import type { GlobalPage } from "@/lib/navigation";
import type { VisibleApps } from "@/types";

interface OverviewProps {
  visibleApps: VisibleApps;
  onOpenApp: (app: AppId) => void;
  onOpenPage: (page: GlobalPage) => void;
  onImport: () => void;
}

function ConnectionCard({
  app,
  onOpen,
  onImport,
}: {
  app: "claude" | "codex";
  onOpen: () => void;
  onImport: () => void;
}) {
  const { t } = useTranslation();
  // A failed read must not be presented as an empty or healthy configuration.
  const query = useQuery({
    queryKey: ["providers", app],
    queryFn: async () => {
      const [providers, currentProviderId] = await Promise.all([
        providersApi.getAll(app),
        providersApi.getCurrent(app),
      ]);
      return { providers, currentProviderId };
    },
  });
  const providers = query.data?.providers ?? {};
  const current = providers[query.data?.currentProviderId ?? ""];
  const count = Object.keys(providers).length;
  return (
    <article className="xb-connection-card">
      <div className="flex items-center justify-between gap-3">
        <span className="xb-app-icon">
          <AppGlyph app={app} size={26} />
        </span>
        <span
          className={
            current && !query.isError
              ? "xb-status xb-status-ready"
              : "xb-status"
          }
        >
          {query.isLoading ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : current && !query.isError ? (
            <Check className="h-3 w-3" />
          ) : (
            <span className="h-1.5 w-1.5 rounded-full bg-current opacity-60" />
          )}
          {query.isLoading
            ? t("workspace.loading")
            : query.isError
              ? t("workspace.readError")
              : current
                ? t("workspace.configured")
                : t("workspace.notConfigured")}
        </span>
      </div>
      <h3 className="mb-1 mt-5 text-[20px] font-semibold tracking-tight">
        {APP_DISPLAY_NAME[app]}
      </h3>
      <p className="text-body text-fg-3">{t(`workspace.${app}Description`)}</p>
      <div className="my-5 rounded-panel bg-subtle px-4 py-3">
        <p className="mb-1 text-caption text-fg-3">
          {t("workspace.currentConfiguration")}
        </p>
        <p className="truncate text-strong" title={current?.name}>
          {query.isLoading
            ? t("workspace.loading")
            : query.isError
              ? t("workspace.readErrorHint")
              : (current?.name ?? t("workspace.noConfiguration"))}
        </p>
      </div>
      <div className="flex items-center justify-between gap-2">
        <span className="text-caption text-fg-3">
          {t("workspace.configurationCount", { count })}
        </span>
        {query.isError ? (
          <Button variant="quiet" onClick={() => void query.refetch()}>
            <RefreshCw className="h-3.5 w-3.5" />
            {t("workspace.retry")}
          </Button>
        ) : (
          <Button variant="quiet" onClick={count ? onOpen : onImport}>
            {count ? t("workspace.manageConnection") : t("workspace.connect")}
            <ArrowRight className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
    </article>
  );
}

export function XbtryOverviewPage({
  visibleApps,
  onOpenApp,
  onOpenPage,
  onImport,
}: OverviewProps) {
  const { t } = useTranslation();
  const primaryApps = (["claude", "codex"] as const).filter(
    (app) => visibleApps[app],
  );
  const shortcuts = [
    {
      page: "usage" as const,
      icon: BarChart3,
      title: "usage",
      detail: "usageHint",
    },
    {
      page: "sessions" as const,
      icon: MessageSquare,
      title: "sessions",
      detail: "sessionsHint",
    },
    {
      page: "mcp" as const,
      icon: Puzzle,
      title: "extensions",
      detail: "extensionsHint",
    },
  ];
  return (
    <>
      <AppPageHeader
        title={t("workspace.title")}
        actions={
          <span className="text-caption text-fg-3">XBTry / Workspace</span>
        }
      />
      <div
        id="main-content"
        className="min-h-0 flex-1 overflow-y-auto scroll-stable"
      >
        <div className="xb-workspace">
          <section aria-labelledby="workspace-connections">
            <div className="mb-4 flex items-center justify-between gap-3">
              <h2 id="workspace-connections" className="text-strong">
                {t("workspace.connections")}
              </h2>
              <button
                className="inline-flex items-center gap-1 text-caption text-fg-3 transition-colors hover:text-fg-1"
                onClick={() => onOpenPage("apps")}
              >
                {t("workspace.allApps")}
                <ChevronRight className="h-3.5 w-3.5" />
              </button>
            </div>
            <div className="xb-connections">
              {primaryApps.map((app) => (
                <ConnectionCard
                  key={app}
                  app={app}
                  onOpen={() => onOpenApp(app)}
                  onImport={onImport}
                />
              ))}
              {primaryApps.length === 0 && (
                <div className="col-span-full rounded-panel border border-dashed p-8 text-center">
                  <p className="mb-4 text-body text-fg-2">
                    {t("workspace.noApps")}
                  </p>
                  <Button variant="neutral" onClick={() => onOpenPage("apps")}>
                    {t("workspace.allApps")}
                  </Button>
                </div>
              )}
            </div>
          </section>

          <section className="xb-key-callout">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl border border-border bg-surface">
              <KeyRound className="h-4 w-4 text-action-text" />
            </div>
            <div className="min-w-0 flex-1">
              <h2 className="text-strong">{t("workspace.keyTitle")}</h2>
              <p className="mt-1 text-caption text-fg-3">
                {t("workspace.keyHint")}
              </p>
            </div>
            <Button variant="quiet" onClick={onImport}>
              {t("workspace.connect")}
              <ArrowRight className="h-4 w-4" />
            </Button>
          </section>

          <section aria-labelledby="workspace-tools" className="mt-8">
            <h2 id="workspace-tools" className="mb-3 text-strong">
              {t("workspace.tools")}
            </h2>
            <div className="xb-shortcuts">
              {shortcuts.map(({ page, icon: Icon, title, detail }) => (
                <button
                  key={page}
                  className="xb-shortcut"
                  onClick={() => onOpenPage(page)}
                >
                  <Icon
                    className="mb-4 h-[18px] w-[18px] text-fg-2"
                    strokeWidth={1.5}
                  />
                  <span className="flex items-center justify-between gap-2 text-body font-medium">
                    {t(`workspace.${title}`)}
                    <ChevronRight className="h-3.5 w-3.5 text-fg-3" />
                  </span>
                  <span className="mt-1 block text-caption text-fg-3">
                    {t(`workspace.${detail}`)}
                  </span>
                </button>
              ))}
            </div>
          </section>
          <p className="mt-7 text-center text-[11px] tracking-wide text-fg-3">
            {t("workspace.footer")}
          </p>
        </div>
      </div>
    </>
  );
}

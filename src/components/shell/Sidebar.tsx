import { useEffect, useRef, useState, type ComponentType } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { getVersion } from "@tauri-apps/api/app";
import {
  ArrowLeft,
  BookOpen,
  ChartColumn,
  ChevronDown,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  Database,
  Folder,
  Globe,
  History,
  House,
  Import,
  Info,
  KeyRound,
  Layers,
  LayoutGrid,
  Route,
  Server,
  Settings,
  SlidersHorizontal,
} from "lucide-react";
import type { AppId } from "@/lib/api";
import type { VisibleApps } from "@/types";
import { orderAppIds } from "@/config/appConfig";
import type { GlobalPage, SettingsSection, View } from "@/lib/navigation";
import { isAppPage } from "@/lib/navigation";
import { SkillsIcon } from "@/components/BrandIcons";
import { useUpdate } from "@/contexts/UpdateContext";
import { sidebarWidth, useSidebarCollapsed } from "@/hooks/useSidebarCollapsed";
import { useMoneyFormat } from "@/hooks/useMoneyFormat";
import { useSettingsQuery } from "@/lib/query";
import {
  fmtInt,
  formatHitRatePercent,
  formatTokensCompact,
  getLocaleFromLanguage,
  getResolvedLang,
  resolveSidebarToday,
  type SidebarTodayItem,
} from "@/components/usage/format";
import { useSidebarStatus, type AppNavStatus } from "@/hooks/useSidebarStatus";
import { HoverTip } from "@/components/ui/hover-tip";
import { DRAG_REGION_ATTR, DRAG_REGION_STYLE, isMac } from "@/lib/platform";
import { cn } from "@/lib/utils";
import { BRAND } from "@/config/brand";
import { APP_DISPLAY_NAME, AppGlyph } from "./AppGlyph";
import { XbtryMark } from "./XbtryMark";

const NO_DRAG = { WebkitAppRegion: "no-drag" } as React.CSSProperties;

type IconComponent = ComponentType<{ className?: string }>;

interface SidebarProps {
  activeApp: AppId;
  view: View;
  visibleApps: VisibleApps;
  settingsSection: SettingsSection;
  onSelectApp: (app: AppId) => void;
  onSelectPage: (page: GlobalPage | "settings") => void;
  onSelectSettingsSection: (section: SettingsSection) => void;
  onExitSettings: () => void;
  /** 打开了「启动时检查应用更新」并且查到了新版本 */
  appsUpdateAvailable?: boolean;
  /** Opens the XBTry key import dialog */
  onOpenXbtryImport?: () => void;
}

type DirectoryProps = SidebarProps & { collapsed: boolean };

/** One navigation for every XBTry workspace, with an optional compact rail. */
export function Sidebar(props: SidebarProps) {
  const { t } = useTranslation();
  const navRef = useRef<HTMLElement>(null);
  const { collapsed, toggle } = useSidebarCollapsed(navRef);
  const { view } = props;
  const inSettings = view === "settings";

  return (
    <nav
      ref={navRef}
      aria-label={t("nav.mainLabel")}
      className={cn(
        "xb-sidebar relative flex h-full shrink-0 flex-col overflow-hidden whitespace-nowrap border-e border-border/70 bg-sidebar text-[13px] text-fg-1",
      )}
      style={{ width: sidebarWidth(collapsed) }}
    >
      <a
        href="#main-content"
        className="absolute -start-[999px] top-2 z-10 rounded-control bg-surface px-2 py-1 text-caption shadow-v7-sm focus:start-2"
      >
        {t("nav.skipToContent")}
      </a>
      <SidebarTopBar
        collapsed={collapsed}
        onToggle={toggle}
        onHome={() => props.onSelectPage("overview")}
      />
      {inSettings ? (
        <SettingsDirectory {...props} collapsed={collapsed} />
      ) : (
        <MainDirectory {...props} collapsed={collapsed} />
      )}
    </nav>
  );
}

function SidebarTopBar({
  collapsed,
  onToggle,
  onHome,
}: {
  collapsed: boolean;
  onToggle: () => void;
  onHome: () => void;
}) {
  const { t } = useTranslation();
  const label = collapsed ? t("nav.expandSidebar") : t("nav.collapseSidebar");
  const Icon = collapsed ? ChevronsRight : ChevronsLeft;

  return (
    <>
      {isMac() && (
        <div
          className="h-10 shrink-0"
          {...DRAG_REGION_ATTR}
          style={DRAG_REGION_STYLE as React.CSSProperties}
        />
      )}
      <div
        className={cn(
          "flex shrink-0 items-center",
          collapsed
            ? "flex-col gap-3 px-3 pb-4 pt-5"
            : "gap-2.5 px-5 pb-6 pt-7",
          isMac() && "!pt-2",
        )}
        {...DRAG_REGION_ATTR}
        style={DRAG_REGION_STYLE as React.CSSProperties}
      >
        <button
          type="button"
          onClick={onHome}
          aria-label={`XBTry · ${t("xbtry.overview", { defaultValue: "工作台" })}`}
          style={NO_DRAG}
          className="flex min-w-0 flex-1 items-center gap-2.5 rounded-lg text-start focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"
        >
          <XbtryMark />
          {!collapsed && (
            <div className="min-w-0">
              <div className="text-[19px] font-semibold leading-6 tracking-[-0.7px]">
                XB<span className="text-action-text">Try</span>
              </div>
              <div className="mt-0.5 text-[10px] font-medium tracking-[0.13em] text-fg-3">
                {t("xbtry.workspaceCaption", { defaultValue: "AI WORKSPACE" })}
              </div>
            </div>
          )}
        </button>
        <HoverTip content={label} side="right" disableHoverableContent>
          <button
            type="button"
            onClick={onToggle}
            aria-label={label}
            style={NO_DRAG}
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
          >
            <Icon className="h-3.5 w-3.5" strokeWidth={1.5} />
          </button>
        </HoverTip>
      </div>
    </>
  );
}

function NavSectionLabel({
  children,
  collapsed,
}: {
  children: React.ReactNode;
  collapsed: boolean;
}) {
  if (collapsed) return <div className="mx-auto my-3 h-px w-6 bg-border/70" />;
  return (
    <div className="mb-2 mt-6 px-6 text-[10px] font-medium uppercase tracking-[0.12em] text-fg-3">
      {children}
    </div>
  );
}

// ─── 主目录 ────────────────────────────────────────────────────────────────

function MainDirectory({
  collapsed,
  activeApp,
  view,
  visibleApps,
  onSelectApp,
  onSelectPage,
  appsUpdateAvailable = false,
  onOpenXbtryImport,
}: DirectoryProps) {
  const { t, i18n } = useTranslation();
  const { moneyShort } = useMoneyFormat();
  const { data: settings } = useSettingsQuery();
  const { hasUpdate } = useUpdate();
  const {
    appStatus,
    todayCost,
    todayTokens,
    todayRequests,
    todayHitRate,
    authNeedsAttention,
  } = useSidebarStatus();
  const apps = orderAppIds(settings?.appOrder).filter(
    (app) => visibleApps[app],
  );
  const onAppPage = isAppPage(view);
  const primaryApps = apps.filter((app) => app === "claude" || app === "codex");
  const additionalApps = apps.filter(
    (app) => app !== "claude" && app !== "codex",
  );
  const additionalAppSelected =
    onAppPage && additionalApps.some((app) => app === activeApp);
  const [moreAppsOpen, setMoreAppsOpen] = useState(additionalAppSelected);
  useEffect(() => {
    if (additionalAppSelected) setMoreAppsOpen(true);
  }, [activeApp, additionalAppSelected]);
  const showAdditionalApps = moreAppsOpen;

  // 选了的项里是 0 的不写（没配定价时金额总是 0）；命中率在今天还没有请求时不写；
  // 全都没有就整个不显示。两项都有时只写数、上下两行（「今日」放进悬停和读屏）：
  // 带上「今日」，换成新台币这类长币种后一行放不下，会把「用量统计」挤成省略号
  const locale = getLocaleFromLanguage(getResolvedLang(i18n));
  const todayValue = (item: SidebarTodayItem): string | null => {
    switch (item) {
      case "cost":
        return todayCost > 0 ? moneyShort(todayCost) : null;
      case "tokens":
        return todayTokens > 0
          ? formatTokensCompact(todayTokens, locale)
          : null;
      case "requests":
        return todayRequests > 0
          ? t("nav.todayRequests", { value: fmtInt(todayRequests, locale) })
          : null;
      case "hitRate":
        return todayRequests > 0
          ? t("nav.todayHitRate", { value: formatHitRatePercent(todayHitRate) })
          : null;
    }
  };
  const todayValues = resolveSidebarToday(settings?.usageDisplay)
    .map(todayValue)
    .filter((part): part is string => part != null);
  const todayText =
    todayValues.length > 0
      ? t("nav.today", { value: todayValues.join(" · ") })
      : undefined;
  const todayTrailing = todayValues.length > 1 ? todayValues : todayText;

  const globals: {
    page: GlobalPage;
    label: string;
    icon: IconComponent;
    trailing?: string | string[];
    trailingLabel?: string;
    alert?: string;
  }[] = [
    { page: "mcp", label: "MCP", icon: Server },
    { page: "skills", label: "Skills", icon: SkillsIcon },
    { page: "prompts", label: t("nav.prompts"), icon: BookOpen },
    { page: "sessions", label: t("nav.sessions"), icon: History },
    {
      page: "auth",
      label: t("nav.auth"),
      icon: KeyRound,
      alert: authNeedsAttention ? t("nav.authNeedsReauth") : undefined,
    },
    {
      page: "usage",
      label: t("nav.usage"),
      icon: ChartColumn,
      trailing: todayTrailing,
      trailingLabel: todayText,
    },
  ];

  const isGlobalSelected = (page: GlobalPage) =>
    view === page || (page === "skills" && view === "skillsDiscovery");

  return (
    <>
      <div
        className={cn(
          "flex min-h-0 flex-1 flex-col overflow-y-auto overflow-x-hidden pb-4",
          collapsed && "no-scrollbar",
        )}
      >
        <NavSectionLabel collapsed={collapsed}>
          {t("xbtry.connectedApps", { defaultValue: "应用连接" })}
        </NavSectionLabel>
        <div
          role="group"
          aria-label={t("nav.appsGroup")}
          className="xb-apps-group flex shrink-0 flex-col gap-0.5"
        >
          {primaryApps.map((app) => (
            <AppNavItem
              key={app}
              app={app}
              collapsed={collapsed}
              selected={onAppPage && activeApp === app}
              status={appStatus(app)}
              onSelect={() => onSelectApp(app)}
            />
          ))}
          {additionalApps.length > 0 && (
            <NavItem
              collapsed={collapsed}
              selected={false}
              icon={showAdditionalApps ? ChevronDown : ChevronRight}
              label={t("xbtry.moreApps", { defaultValue: "更多应用" })}
              expanded={showAdditionalApps}
              onClick={() => setMoreAppsOpen(!showAdditionalApps)}
            />
          )}
          {showAdditionalApps &&
            additionalApps.map((app) => (
              <AppNavItem
                key={app}
                app={app}
                collapsed={collapsed}
                selected={onAppPage && activeApp === app}
                status={appStatus(app)}
                onSelect={() => onSelectApp(app)}
              />
            ))}
          <NavItem
            collapsed={collapsed}
            selected={view === "apps"}
            icon={LayoutGrid}
            label={t("nav.apps")}
            title={
              appsUpdateAvailable ? t("nav.appsHasUpdate") : t("nav.appsTitle")
            }
            dot={appsUpdateAvailable}
            onClick={() => onSelectPage("apps")}
          />
        </div>
        <NavItem
          collapsed={collapsed}
          selected={view === "overview"}
          icon={House}
          label={t("xbtry.overview", { defaultValue: "工作台" })}
          onClick={() => onSelectPage("overview")}
        />
        {onOpenXbtryImport && (
          <NavItem
            collapsed={collapsed}
            selected={false}
            icon={Import}
            label={t("xbtry.navImport")}
            onClick={onOpenXbtryImport}
          />
        )}
        <NavSectionLabel collapsed={collapsed}>
          {t("xbtry.workspaceTools", { defaultValue: "工作区" })}
        </NavSectionLabel>
        <div
          role="group"
          aria-label={t("xbtry.workspaceTools", { defaultValue: "工作区" })}
          className="flex shrink-0 flex-col gap-0.5"
        >
          {globals.map((item) => (
            <NavItem
              key={item.page}
              collapsed={collapsed}
              selected={isGlobalSelected(item.page)}
              icon={item.icon}
              label={item.label}
              trailing={item.trailing}
              trailingLabel={item.trailingLabel}
              alert={item.alert}
              onClick={() => onSelectPage(item.page)}
            />
          ))}
        </div>
      </div>
      <div className="mx-5 h-px shrink-0 bg-border/70" />
      <div className="flex shrink-0 flex-col py-3">
        <NavItem
          collapsed={collapsed}
          selected={false}
          icon={Settings}
          label={t("nav.settings")}
          title={hasUpdate ? t("nav.settingsHasUpdate") : t("nav.settings")}
          dot={hasUpdate}
          dotTone="success"
          onClick={() => onSelectPage("settings")}
        />
      </div>
    </>
  );
}

function modeTag(
  status: AppNavStatus,
  t: (key: string) => string,
): { label: string; className: string } | null {
  if (status.mode === "route") {
    return {
      label: t("nav.mode.route"),
      className: "bg-route-soft text-route-text",
    };
  }
  if (status.mode === "stack") {
    return {
      label: t("nav.mode.stack"),
      className: "bg-stack-soft text-stack-text",
    };
  }
  return null;
}

function AppNavItem({
  app,
  collapsed,
  selected,
  status,
  onSelect,
}: {
  app: AppId;
  collapsed: boolean;
  selected: boolean;
  status: AppNavStatus;
  onSelect: () => void;
}) {
  const { t } = useTranslation();
  const name = APP_DISPLAY_NAME[app];
  const tag = modeTag(status, t);
  const modeName =
    status.mode === "route"
      ? t("nav.mode.route")
      : status.mode === "stack"
        ? t("nav.mode.stack")
        : null;
  const tip = status.alert
    ? `${name} · ${t("nav.needsAttention")}`
    : modeName
      ? `${name} · ${modeName}`
      : name;
  const badgeBg = selected ? "bg-selected" : "bg-sidebar";

  if (collapsed) {
    const marker = status.alert
      ? { className: "bg-danger text-action-fg", icon: AlertGlyph }
      : status.mode === "stack"
        ? { className: "bg-stack-solid text-stack-on", icon: Layers }
        : status.mode === "route"
          ? { className: "bg-route-solid text-route-on", icon: Route }
          : null;
    return (
      <HoverTip content={tip} side="right" disableHoverableContent>
        <button
          type="button"
          onClick={onSelect}
          aria-label={tip}
          aria-current={selected ? "page" : undefined}
          className={cn(
            "xb-nav-item mx-auto flex h-10 w-11 shrink-0 items-center justify-center rounded-xl text-fg-2 transition-colors hover:bg-subtle hover:text-fg-1",
            selected && "bg-action-soft text-action-text hover:bg-action-soft",
          )}
        >
          <span className="relative flex">
            <AppGlyph app={app} size={20} badgeClassName={badgeBg} />
            {marker && (
              <span
                aria-hidden="true"
                className={cn(
                  "absolute -end-[7px] -top-1.5 flex h-3.5 w-3.5 items-center justify-center rounded-full border-2",
                  selected ? "border-selected" : "border-sidebar",
                  marker.className,
                )}
              >
                <marker.icon className="h-2 w-2" strokeWidth={3} />
              </span>
            )}
          </span>
        </button>
      </HoverTip>
    );
  }

  return (
    <button
      type="button"
      onClick={onSelect}
      aria-current={selected ? "page" : undefined}
      className={cn(
        "xb-nav-item mx-3 flex h-9 w-[188px] shrink-0 items-center gap-3 rounded-lg px-3 text-start text-fg-2 transition-colors hover:bg-subtle hover:text-fg-1",
        selected &&
          "bg-action-soft font-medium text-action-text hover:bg-action-soft",
      )}
    >
      <AppGlyph app={app} size={16} badgeClassName={badgeBg} />
      <span className="min-w-0 flex-1 truncate">{name}</span>
      {status.alert ? (
        <span
          role="img"
          aria-label={t("nav.needsAttention")}
          className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger"
        />
      ) : (
        // 选中的应用不再显示模式标签：页头下面的模式行已经写明
        tag &&
        !selected && (
          <span
            className={cn(
              "h-[18px] whitespace-nowrap rounded-md px-1.5 text-[10px] leading-[18px]",
              tag.className,
            )}
          >
            {tag.label}
          </span>
        )
      )}
    </button>
  );
}

function AlertGlyph({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={3.5}
      strokeLinecap="round"
      className={className}
    >
      <path d="M12 4v10" />
      <path d="M12 20h.01" />
    </svg>
  );
}

function NavItem({
  collapsed,
  compact = false,
  selected,
  icon: Icon,
  label,
  title,
  trailing,
  trailingLabel,
  alert,
  dot = false,
  dotTone = "neutral",
  endDot = false,
  expanded,
  onClick,
}: {
  collapsed: boolean;
  /** 底栏两格并排：宽度由父级平分 */
  compact?: boolean;
  selected: boolean;
  icon: IconComponent;
  label: string;
  title?: string;
  /** 行尾的小字；放不下时在空格处折成两行，给数组时直接上下排 */
  trailing?: string | string[];
  /** 行尾小字的完整说法（读屏和悬停用），行尾只写了数时给 */
  trailingLabel?: string;
  /** 红点：需要处理，内容是给读屏的说明 */
  alert?: string;
  /** 圆点：有更新 */
  dot?: boolean;
  /** The desktop app uses a subtle green update dot. */
  dotTone?: "neutral" | "success";
  /** 圆点放在行尾（设置目录里的「关于」），而不是图标角上 */
  endDot?: boolean;
  expanded?: boolean;
  onClick: () => void;
}) {
  const ring = selected ? "border-selected" : "border-sidebar";
  const dotFill = dotTone === "success" ? "bg-success" : "bg-fg-1";
  const trailingLines =
    trailing == null ? [] : Array.isArray(trailing) ? trailing : [trailing];
  const accessibleName = [
    label,
    trailingLabel ?? trailingLines.join(" · "),
    alert,
  ]
    .filter(Boolean)
    .join(" · ");
  const glyph = (size: number) => (
    <span
      aria-hidden="true"
      className={cn(
        "relative flex shrink-0",
        selected ? "text-action-text" : "text-fg-3",
      )}
    >
      <Icon className={size === 20 ? "h-5 w-5" : "h-[18px] w-[18px]"} />
      {dot && (collapsed || !endDot) && (
        <span
          className={cn(
            "absolute -end-1 -top-[3px] h-2.5 w-2.5 rounded-full border-2",
            dotFill,
            ring,
          )}
        />
      )}
      {collapsed && alert && (
        <span
          className={cn(
            "absolute -end-[7px] -top-1.5 flex h-3.5 w-3.5 items-center justify-center rounded-full border-2 bg-danger text-action-fg",
            ring,
          )}
        >
          <AlertGlyph className="h-2 w-2" />
        </span>
      )}
    </span>
  );

  if (collapsed) {
    return (
      <HoverTip
        content={title ?? accessibleName}
        side="right"
        disableHoverableContent
      >
        <button
          type="button"
          onClick={onClick}
          aria-label={title ?? accessibleName}
          aria-current={selected ? "page" : undefined}
          aria-expanded={expanded}
          className={cn(
            "xb-nav-item mx-auto flex h-10 w-11 shrink-0 items-center justify-center rounded-xl text-fg-2 transition-colors hover:bg-subtle hover:text-fg-1",
            selected && "bg-action-soft text-action-text hover:bg-action-soft",
          )}
        >
          {glyph(20)}
        </button>
      </HoverTip>
    );
  }

  // 展开时名字可见；只有 title 比名字多说了点什么（如「有可用更新」），
  // 或行尾只写了数（完整说法在 trailingLabel）时才挂提示
  const hoverContent =
    title && title !== label
      ? title
      : trailingLabel
        ? accessibleName
        : undefined;
  return (
    <HoverTip
      content={hoverContent}
      side={compact ? "top" : "right"}
      disableHoverableContent
    >
      <button
        type="button"
        onClick={onClick}
        aria-label={trailingLabel ? accessibleName : undefined}
        aria-current={selected ? "page" : undefined}
        aria-expanded={expanded}
        className={cn(
          "xb-nav-item flex h-9 shrink-0 items-center gap-3 rounded-lg px-3 text-start text-fg-2 transition-colors hover:bg-subtle hover:text-fg-1",
          compact ? "min-w-0 flex-1" : "mx-3 w-[188px]",
          selected &&
            "bg-action-soft font-medium text-action-text hover:bg-action-soft",
        )}
      >
        {glyph(18)}
        {trailingLines.length === 0 ? (
          <span className="min-w-0 flex-1 truncate">{label}</span>
        ) : (
          <>
            {/* 有行尾小字时名字不让：放不下就让小字折行（两行正好塞进 28px 的行高）。
                裸文字贴着行内距会比模式标签的字更靠边，右边让出标签的 px-1.5 和它对齐 */}
            <span className="shrink-0 grow">{label}</span>
            <span className="min-w-0 pe-1.5 text-end text-caption leading-[13px] tabular-nums text-fg-3">
              {trailingLines.map((line) => (
                <span
                  key={line}
                  className={
                    trailingLines.length > 1
                      ? "block whitespace-nowrap"
                      : undefined
                  }
                >
                  {line}
                </span>
              ))}
            </span>
          </>
        )}
        {alert && (
          <span
            role="img"
            aria-label={alert}
            title={alert}
            className="h-1.5 w-1.5 shrink-0 rounded-full bg-danger"
          />
        )}
        {dot && endDot && (
          <span
            aria-hidden="true"
            className={cn("h-1.5 w-1.5 shrink-0 rounded-full", dotFill)}
          />
        )}
      </button>
    </HoverTip>
  );
}

// ─── 设置目录 ──────────────────────────────────────────────────────────────

const SETTINGS_ITEMS: { section: SettingsSection; icon: IconComponent }[] = [
  { section: "general", icon: SlidersHorizontal },
  { section: "appConfig", icon: Folder },
  { section: "routing", icon: Route },
  { section: "network", icon: Globe },
  { section: "data", icon: Database },
  { section: "about", icon: Info },
];

function SettingsDirectory({
  collapsed,
  settingsSection,
  onSelectSettingsSection,
  onExitSettings,
}: DirectoryProps) {
  const { t } = useTranslation();
  const { hasUpdate } = useUpdate();
  const { data: version } = useQuery({
    queryKey: ["app-version"],
    queryFn: () => getVersion(),
    staleTime: Infinity,
  });

  return (
    <>
      {collapsed ? (
        <NavItem
          collapsed
          selected={false}
          icon={ArrowLeft}
          label={t("nav.back")}
          onClick={onExitSettings}
        />
      ) : (
        <button
          type="button"
          onClick={onExitSettings}
          className="mx-3 flex h-9 w-[188px] shrink-0 items-center gap-3 rounded-lg px-3 text-start text-fg-2 transition-colors hover:bg-subtle hover:text-fg-1"
        >
          <ArrowLeft className="h-4 w-4" strokeWidth={1.5} aria-hidden="true" />
          {t("nav.back")}
        </button>
      )}
      {!collapsed && (
        <div className="mb-2 mt-6 px-6 text-[10px] font-medium uppercase tracking-[0.12em] text-fg-3">
          {t("nav.settings")}
        </div>
      )}
      <div
        role="group"
        aria-label={t("nav.settings")}
        className={cn(
          "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto",
          collapsed && "mt-2",
        )}
      >
        {SETTINGS_ITEMS.map(({ section, icon }) => (
          <NavItem
            key={section}
            collapsed={collapsed}
            selected={settingsSection === section}
            icon={icon}
            label={t(`settings.sections.${section}`)}
            dot={section === "about" && hasUpdate}
            dotTone="success"
            endDot
            title={
              section === "about" && hasUpdate
                ? t("nav.settingsHasUpdate")
                : undefined
            }
            onClick={() => onSelectSettingsSection(section)}
          />
        ))}
      </div>
      {!collapsed && version && (
        <div className="shrink-0 px-6 pb-5 text-[11px] text-fg-3">
          {BRAND.name} v{version}
        </div>
      )}
    </>
  );
}

export type { SidebarProps };

import { useEffect, useState, type FormEvent } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Eye, EyeOff, Loader2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { Switch } from "@/components/ui/switch";
import { APP_DISPLAY_NAME, AppGlyph } from "@/components/shell/AppGlyph";
import { providersApi, settingsApi } from "@/lib/api";
import { toast } from "@/lib/toast";
import { BRAND } from "@/config/brand";
import { cn } from "@/lib/utils";
import {
  ENDPOINT,
  importInto,
  TARGETS,
  type Target,
} from "@/components/xbtry/importCore";

interface XbtryImportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function XbtryImportDialog({
  open,
  onOpenChange,
}: XbtryImportDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [apiKey, setApiKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [targets, setTargets] = useState<Record<Target, boolean>>({
    claude: true,
    codex: false,
  });
  const [activate, setActivate] = useState(true);
  const [importing, setImporting] = useState(false);
  const [touched, setTouched] = useState(false);
  const [failedTargets, setFailedTargets] = useState<Target[]>([]);

  useEffect(() => {
    if (!open) {
      setApiKey("");
      setShowKey(false);
      setTouched(false);
      setFailedTargets([]);
    }
  }, [open]);

  const key = apiKey.trim();
  const keyError = !key
    ? t("xbtry.keyRequired")
    : /\s/.test(key)
      ? t("xbtry.keyInvalid")
      : null;
  const selected = TARGETS.filter((app) => targets[app]);
  const canSubmit = !keyError && selected.length > 0 && !importing;

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setTouched(true);
    if (!canSubmit) return;
    setImporting(true);
    setFailedTargets([]);
    const done: Target[] = [];
    const failed: Target[] = [];
    try {
      for (const app of selected) {
        try {
          await importInto(app, key, activate);
          done.push(app);
        } catch (error) {
          failed.push(app);
          console.error("[XbtryImport] failed", app, error);
          toast.error(t("xbtry.failed", { app: APP_DISPLAY_NAME[app] }), {
            description: String(error),
            closeButton: true,
          });
        }
        // A failed cache refresh must not interrupt the other selected import.
        try {
          await queryClient.invalidateQueries({ queryKey: ["providers", app] });
        } catch (error) {
          console.error("[XbtryImport] provider refresh failed", app, error);
        }
      }
      try {
        await providersApi.updateTrayMenu();
      } catch (error) {
        console.error("[XbtryImport] tray refresh failed", error);
      }

      if (done.length > 0) {
        toast.success(
          t("xbtry.success", {
            apps: done.map((app) => APP_DISPLAY_NAME[app]).join("、"),
          }),
          { closeButton: true },
        );
      }
      if (failed.length === 0) {
        setApiKey("");
        setShowKey(false);
        onOpenChange(false);
      } else {
        // Keep the key and failed targets available for a focused retry.
        setFailedTargets(failed);
        setTargets({
          claude: failed.includes("claude"),
          codex: failed.includes("codex"),
        });
      }
    } finally {
      setImporting(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !importing && onOpenChange(next)}
    >
      <DialogContent className="xb-import-dialog overflow-y-auto sm:max-w-[480px]">
        <DialogHeader className="gap-2 pb-6 pt-7 text-left">
          <span className="text-[11px] font-semibold tracking-[0.18em] text-fg-3">
            XBTRY
          </span>
          <DialogTitle className="text-[23px] tracking-tight">
            {t("xbtry.title")}
          </DialogTitle>
          <DialogDescription className="text-[13px] leading-relaxed text-fg-3">
            {t("xbtry.description")}
          </DialogDescription>
        </DialogHeader>

        <form onSubmit={handleSubmit} className="flex flex-col gap-6 px-6 pb-6">
          <fieldset className="min-w-0" disabled={importing}>
            <legend className="mb-3 flex w-full items-center justify-between text-body font-medium text-fg-1">
              {t("xbtry.targets")}
              <span className="text-caption font-normal text-fg-3">
                {t("xbtry.multipleTargets", { defaultValue: "可多选" })}
              </span>
            </legend>
            <div className="grid grid-cols-2 gap-3">
              {TARGETS.map((app) => (
                <label
                  key={app}
                  className={cn(
                    "flex min-w-0 cursor-pointer flex-col gap-3 rounded-xl border border-border bg-surface p-4 transition-colors hover:bg-subtle",
                    targets[app] && "border-action bg-action-soft",
                    importing && "cursor-wait opacity-60",
                  )}
                >
                  <span className="flex items-center justify-between">
                    <AppGlyph app={app} size={24} />
                    <Checkbox
                      aria-label={APP_DISPLAY_NAME[app]}
                      checked={targets[app]}
                      onCheckedChange={(checked) =>
                        setTargets((prev) => ({ ...prev, [app]: checked }))
                      }
                    />
                  </span>
                  <span className="min-w-0">
                    <span className="block text-body font-medium text-fg-1">
                      {APP_DISPLAY_NAME[app]}
                    </span>
                    <span className="mt-1 block truncate text-[11px] text-fg-3">
                      {ENDPOINT[app].replace(/^https?:\/\//, "")}
                    </span>
                  </span>
                </label>
              ))}
            </div>
            {touched && selected.length === 0 && (
              <p role="alert" className="mt-2 text-caption text-danger-text">
                {t("xbtry.targetRequired")}
              </p>
            )}
          </fieldset>

          <div className="flex flex-col gap-2.5">
            <div className="flex items-center justify-between">
              <label
                htmlFor="xbtry-api-key"
                className="text-body font-medium text-fg-1"
              >
                {t("xbtry.apiKey")}
              </label>
              <button
                type="button"
                onClick={() => void settingsApi.openExternal(BRAND.keyUrl)}
                className="inline-flex items-center gap-1 text-caption text-fg-3 transition-colors hover:text-action-text"
              >
                {t("xbtry.getKey")}
                <ExternalLink className="h-3 w-3" />
              </button>
            </div>
            <div className="relative">
              <Input
                id="xbtry-api-key"
                autoFocus
                autoComplete="off"
                type={showKey ? "text" : "password"}
                value={apiKey}
                disabled={importing}
                onChange={(event) => setApiKey(event.target.value)}
                onBlur={() => setTouched(true)}
                placeholder="sk-…"
                spellCheck={false}
                aria-invalid={touched && !!keyError}
                aria-describedby={
                  touched && keyError ? "xbtry-key-error" : undefined
                }
                className="h-11 rounded-lg pe-11 font-mono"
              />
              <button
                type="button"
                onClick={() => setShowKey((value) => !value)}
                aria-label={showKey ? t("xbtry.hideKey") : t("xbtry.showKey")}
                aria-pressed={showKey}
                className="absolute end-1 top-1 flex h-9 w-9 items-center justify-center rounded-md text-fg-3 transition-colors hover:bg-subtle hover:text-fg-1"
              >
                {showKey ? (
                  <EyeOff className="h-4 w-4" />
                ) : (
                  <Eye className="h-4 w-4" />
                )}
              </button>
            </div>
            {touched && keyError && (
              <p
                id="xbtry-key-error"
                role="alert"
                className="text-caption text-danger-text"
              >
                {keyError}
              </p>
            )}
          </div>

          <div className="flex items-center justify-between gap-5 rounded-lg bg-subtle px-4 py-3.5">
            <div className="min-w-0">
              <p className="text-body font-medium text-fg-1">
                {t("xbtry.activate")}
              </p>
              <p className="mt-1 text-[11px] leading-relaxed text-fg-3">
                {t("xbtry.activateHint")}
              </p>
            </div>
            <Switch
              checked={activate}
              disabled={importing}
              onCheckedChange={setActivate}
              aria-label={t("xbtry.activate")}
            />
          </div>

          {failedTargets.length > 0 && (
            <p
              role="alert"
              className="text-caption leading-relaxed text-danger-text"
            >
              {t("xbtry.retryFailed", {
                defaultValue: "{{apps}} 接入失败，已保留密钥，可直接重试。",
                apps: failedTargets
                  .map((app) => APP_DISPLAY_NAME[app])
                  .join("、"),
              })}
            </p>
          )}

          <div className="flex gap-3 border-t border-border pt-5">
            <Button
              type="button"
              variant="neutral"
              onClick={() => onOpenChange(false)}
              disabled={importing}
              className="h-10 rounded-lg px-5"
            >
              {t("xbtry.cancel")}
            </Button>
            <Button
              type="submit"
              variant="solid"
              disabled={importing}
              className="h-10 flex-1 rounded-lg"
            >
              {importing && <Loader2 className="h-4 w-4 animate-spin" />}
              {importing ? t("xbtry.importing") : t("xbtry.import")}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}

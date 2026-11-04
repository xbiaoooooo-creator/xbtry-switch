import { providersApi } from "@/lib/api";
import { deeplinkApi } from "@/lib/api/deeplink";
import { BRAND } from "@/config/brand";
import type { Provider } from "@/types";

export type Target = "claude" | "codex";
export const TARGETS: Target[] = ["codex", "claude"];

export const ENDPOINT: Record<Target, string> = {
  claude: BRAND.claude.baseUrl,
  codex: BRAND.codex.baseUrl,
};

/** An existing XBTry row: same display name, or a config pointing at the relay host. */
function isXbtryProvider(provider: Provider, app: Target): boolean {
  if (provider.name === BRAND.name) return true;
  const host = new URL(ENDPOINT[app]).host;
  const cfg = provider.settingsConfig ?? {};
  const target =
    app === "claude" ? cfg.env?.ANTHROPIC_BASE_URL : (cfg.config as unknown);
  return typeof target === "string" && target.includes(host);
}

function withKey(provider: Provider, app: Target, key: string): Provider {
  const cfg = structuredClone(provider.settingsConfig ?? {});
  if (app === "claude") {
    const env = { ...(cfg.env ?? {}) };
    if ("ANTHROPIC_API_KEY" in env && !("ANTHROPIC_AUTH_TOKEN" in env)) {
      env.ANTHROPIC_API_KEY = key;
    } else {
      env.ANTHROPIC_AUTH_TOKEN = key;
    }
    cfg.env = env;
  } else {
    cfg.auth = { ...(cfg.auth ?? {}), OPENAI_API_KEY: key };
  }
  return { ...provider, settingsConfig: cfg };
}

/**
 * Re-importing replaces the key on the existing XBTry row instead of adding a
 * duplicate, so customers can paste a renewed key the same way.
 */
export async function importInto(app: Target, key: string, activate: boolean) {
  const providers = await providersApi.getAll(app);
  const existing = Object.values(providers).find((p) =>
    isXbtryProvider(p, app),
  );
  if (existing) {
    await providersApi.update(withKey(existing, app, key), app);
    if (activate && (await providersApi.getCurrent(app)) !== existing.id) {
      await providersApi.switch(existing.id, app);
    }
    return;
  }
  await deeplinkApi.importFromDeeplink({
    version: "v1",
    resource: "provider",
    app,
    name: BRAND.name,
    homepage: BRAND.website,
    endpoint: ENDPOINT[app],
    apiKey: key,
    model: app === "claude" ? BRAND.claude.model : BRAND.codex.model,
    apiFormat: app === "claude" ? "anthropic" : "openai_responses",
    enabled: activate,
  });
}
